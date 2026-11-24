use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use apitester_auth::{apply_auth, AuthConfig};
use apitester_core::models::{AppSettings, RequestItem};
use apitester_core::variables::{VariableContext, VariableResolver};
use apitester_net::{HttpClient, HttpRequestOptions};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use crate::collection_runner::{build_auth_context, resolve_auth_variables};

/// Virtual-user concurrency profile for a load test.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LoadProfile {
    Constant {
        vus: usize,
    },
    Ramp {
        start_vus: usize,
        end_vus: usize,
    },
    Spike {
        base_vus: usize,
        spike_vus: usize,
        spike_at_pct: f32,
        spike_duration_pct: f32,
    },
    Peak {
        base_vus: usize,
        peak_vus: usize,
    },
}

impl LoadProfile {
    /// Returns the target active virtual users at normalized progress `0.0..=1.0`.
    pub fn target_vus(&self, progress: f32) -> usize {
        let p = progress.clamp(0.0, 1.0);
        match *self {
            Self::Constant { vus } => vus,
            Self::Ramp { start_vus, end_vus } => lerp_usize(start_vus, end_vus, p),
            Self::Spike {
                base_vus,
                spike_vus,
                spike_at_pct,
                spike_duration_pct,
            } => {
                let start = spike_at_pct.clamp(0.0, 1.0);
                let end = (start + spike_duration_pct.max(0.0)).min(1.0);
                if p >= start && p <= end {
                    spike_vus
                } else {
                    base_vus
                }
            }
            Self::Peak { base_vus, peak_vus } => {
                if p <= 0.5 {
                    lerp_usize(base_vus, peak_vus, p * 2.0)
                } else {
                    lerp_usize(peak_vus, base_vus, (p - 0.5) * 2.0)
                }
            }
        }
    }

    /// Returns the maximum virtual users required across the profile.
    pub fn max_vus(&self) -> usize {
        match *self {
            Self::Constant { vus } => vus,
            Self::Ramp { start_vus, end_vus } => start_vus.max(end_vus),
            Self::Spike {
                base_vus,
                spike_vus,
                ..
            } => base_vus.max(spike_vus),
            Self::Peak { base_vus, peak_vus } => base_vus.max(peak_vus),
        }
    }
}

/// Configuration for [`run_load_test`].
#[derive(Debug, Clone)]
pub struct LoadTestConfig {
    pub duration_secs: u64,
    pub profile: LoadProfile,
    pub settings: AppSettings,
    pub cancel: Arc<AtomicBool>,
}

impl Default for LoadTestConfig {
    fn default() -> Self {
        Self {
            duration_secs: 10,
            profile: LoadProfile::Constant { vus: 5 },
            settings: AppSettings::default(),
            cancel: Arc::new(AtomicBool::new(false)),
        }
    }
}

/// A 1-second metrics bucket for charting load test throughput and latency over time.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LoadTimelinePoint {
    pub second: u64,
    pub active_vus: usize,
    pub requests: u64,
    pub errors: u64,
    pub avg_latency_ms: f64,
    pub p95_latency_ms: f64,
}

/// Live progress snapshot streamed during a load test.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoadProgressUpdate {
    pub elapsed_secs: f64,
    pub active_vus: usize,
    pub total_requests: u64,
    pub error_count: u64,
    pub rps: f64,
    pub latest_bucket: LoadTimelinePoint,
}

/// Final aggregated latency, throughput, and time-series report for a load test.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LoadTestSummary {
    pub total_requests: u64,
    pub error_count: u64,
    pub error_rate: f64,
    pub rps: f64,
    pub min_ms: f64,
    pub max_ms: f64,
    pub avg_ms: f64,
    pub p50_ms: f64,
    pub p90_ms: f64,
    pub p95_ms: f64,
    pub p99_ms: f64,
    pub duration_secs: f64,
    pub timeline: Vec<LoadTimelinePoint>,
}

#[derive(Default)]
struct MetricsCollector {
    all_latencies: Vec<f64>,
    total_requests: u64,
    error_count: u64,
    bucket_latencies: Vec<f64>,
    bucket_requests: u64,
    bucket_errors: u64,
}

impl MetricsCollector {
    fn record(&mut self, latency_ms: f64, is_error: bool) {
        self.all_latencies.push(latency_ms);
        self.total_requests += 1;
        if is_error {
            self.error_count += 1;
        }
        self.bucket_latencies.push(latency_ms);
        self.bucket_requests += 1;
        if is_error {
            self.bucket_errors += 1;
        }
    }

    fn flush_bucket(&mut self, second: u64, active_vus: usize) -> (LoadTimelinePoint, u64, u64) {
        let mut lats = std::mem::take(&mut self.bucket_latencies);
        let requests = std::mem::take(&mut self.bucket_requests);
        let errors = std::mem::take(&mut self.bucket_errors);

        let (avg_latency_ms, p95_latency_ms) = if lats.is_empty() {
            (0.0, 0.0)
        } else {
            lats.sort_by(|a, b| a.total_cmp(b));
            let avg = lats.iter().sum::<f64>() / lats.len() as f64;
            let p95 = percentile(&lats, 95.0);
            (avg, p95)
        };

        let point = LoadTimelinePoint {
            second,
            active_vus,
            requests,
            errors,
            avg_latency_ms,
            p95_latency_ms,
        };
        (point, self.total_requests, self.error_count)
    }
}

/// Runs a native async load test against `request` using lightweight `tokio` virtual user tasks.
pub async fn run_load_test(
    request: RequestItem,
    vars: VariableContext,
    auth: AuthConfig,
    config: LoadTestConfig,
    progress_tx: Option<mpsc::UnboundedSender<LoadProgressUpdate>>,
) -> LoadTestSummary {
    let duration_secs = config.duration_secs.max(1);
    let max_vus = config.profile.max_vus().max(1);

    let resolver = VariableResolver::new(vars.clone());
    let resolved_auth = resolve_auth_variables(&auth, &resolver);
    let auth_ctx = build_auth_context(&request, &resolver);
    let auth_app = apply_auth(&resolved_auth, &auth_ctx).unwrap_or_default();

    let http_opts = Arc::new(HttpRequestOptions {
        extra_headers: auth_app.headers,
        extra_query: auth_app.query_params,
        cookies: Vec::new(),
        settings: config.settings.clone(),
    });
    let shared_req = Arc::new(request);
    let shared_vars = Arc::new(vars);
    let metrics = Arc::new(Mutex::new(MetricsCollector::default()));
    let stop_flag = Arc::new(AtomicBool::new(false));
    let target_vus = Arc::new(AtomicUsize::new(config.profile.target_vus(0.0)));

    let mut handles = Vec::with_capacity(max_vus);
    for vu_id in 0..max_vus {
        let req = Arc::clone(&shared_req);
        let vars = Arc::clone(&shared_vars);
        let opts = Arc::clone(&http_opts);
        let metrics = Arc::clone(&metrics);
        let stop = Arc::clone(&stop_flag);
        let cancel = Arc::clone(&config.cancel);
        let target = Arc::clone(&target_vus);

        handles.push(tokio::spawn(async move {
            let client = HttpClient::new();
            while !stop.load(Ordering::Relaxed) && !cancel.load(Ordering::Relaxed) {
                if vu_id >= target.load(Ordering::Relaxed) {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                    continue;
                }

                let started = Instant::now();
                let res = client.execute(&req, &vars, &opts).await;
                let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
                let is_error = match res {
                    Ok(resp) => resp.status >= 400,
                    Err(_) => true,
                };

                if let Ok(mut guard) = metrics.lock() {
                    guard.record(elapsed_ms, is_error);
                }
            }
        }));
    }

    let started = Instant::now();
    let total_duration = Duration::from_secs(duration_secs);
    let mut timeline = Vec::with_capacity(duration_secs as usize);
    let mut next_bucket_sec = 1u64;

    while started.elapsed() < total_duration && !config.cancel.load(Ordering::Relaxed) {
        let elapsed = started.elapsed();
        let progress = (elapsed.as_secs_f32() / duration_secs as f32).clamp(0.0, 1.0);
        let desired_vus = config.profile.target_vus(progress);
        target_vus.store(desired_vus, Ordering::Relaxed);

        if elapsed.as_secs() >= next_bucket_sec {
            let (bucket, total_reqs, err_count) = metrics
                .lock()
                .map(|mut m| m.flush_bucket(next_bucket_sec, desired_vus))
                .unwrap_or_else(|_| (LoadTimelinePoint::default(), 0, 0));
            let elapsed_secs = elapsed.as_secs_f64().max(0.001);
            timeline.push(bucket.clone());

            if let Some(tx) = &progress_tx {
                let _ = tx.send(LoadProgressUpdate {
                    elapsed_secs,
                    active_vus: desired_vus,
                    total_requests: total_reqs,
                    error_count: err_count,
                    rps: total_reqs as f64 / elapsed_secs,
                    latest_bucket: bucket,
                });
            }
            next_bucket_sec += 1;
        }

        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    stop_flag.store(true, Ordering::Relaxed);
    for handle in handles {
        let _ = handle.await;
    }

    let final_elapsed_secs = started.elapsed().as_secs_f64().max(0.001);

    let mut guard = metrics.lock().unwrap_or_else(|e| e.into_inner());
    if guard.bucket_requests > 0 {
        let active_vus = target_vus.load(Ordering::Relaxed);
        let (bucket, _, _) = guard.flush_bucket(next_bucket_sec, active_vus);
        timeline.push(bucket);
    }

    let total_requests = guard.total_requests;
    let error_count = guard.error_count;
    let mut lats = std::mem::take(&mut guard.all_latencies);
    lats.sort_by(|a, b| a.total_cmp(b));

    let (min_ms, max_ms, avg_ms, p50_ms, p90_ms, p95_ms, p99_ms) = if lats.is_empty() {
        (0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0)
    } else {
        let min = lats[0];
        let max = *lats.last().unwrap_or(&0.0);
        let avg = lats.iter().sum::<f64>() / lats.len() as f64;
        (
            min,
            max,
            avg,
            percentile(&lats, 50.0),
            percentile(&lats, 90.0),
            percentile(&lats, 95.0),
            percentile(&lats, 99.0),
        )
    };

    let error_rate = if total_requests == 0 {
        0.0
    } else {
        error_count as f64 / total_requests as f64
    };

    LoadTestSummary {
        total_requests,
        error_count,
        error_rate,
        rps: total_requests as f64 / final_elapsed_secs,
        min_ms,
        max_ms,
        avg_ms,
        p50_ms,
        p90_ms,
        p95_ms,
        p99_ms,
        duration_secs: final_elapsed_secs,
        timeline,
    }
}

fn lerp_usize(start: usize, end: usize, t: f32) -> usize {
    let s = start as f32;
    let e = end as f32;
    (s + (e - s) * t).round().max(0.0) as usize
}

fn percentile(sorted: &[f64], pct: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let idx = ((pct / 100.0) * (sorted.len() - 1) as f64).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

#[cfg(test)]
mod tests {
    use super::*;
    use apitester_core::models::HttpMethod;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    #[test]
    fn load_profiles_compute_expected_target_vus() {
        assert_eq!(LoadProfile::Constant { vus: 10 }.target_vus(0.5), 10);

        let ramp = LoadProfile::Ramp {
            start_vus: 2,
            end_vus: 10,
        };
        assert_eq!(ramp.target_vus(0.0), 2);
        assert_eq!(ramp.target_vus(0.5), 6);
        assert_eq!(ramp.target_vus(1.0), 10);

        let spike = LoadProfile::Spike {
            base_vus: 2,
            spike_vus: 20,
            spike_at_pct: 0.4,
            spike_duration_pct: 0.2,
        };
        assert_eq!(spike.target_vus(0.1), 2);
        assert_eq!(spike.target_vus(0.5), 20);
        assert_eq!(spike.target_vus(0.8), 2);

        let peak = LoadProfile::Peak {
            base_vus: 2,
            peak_vus: 12,
        };
        assert_eq!(peak.target_vus(0.0), 2);
        assert_eq!(peak.target_vus(0.5), 12);
        assert_eq!(peak.target_vus(1.0), 2);
    }

    #[tokio::test]
    async fn executes_load_test_and_collects_percentiles_and_timeline() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");

        let server_handle = tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let mut buf = [0u8; 1024];
                    let _ = stream.read(&mut buf).await;
                    let resp = "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok";
                    let _ = stream.write_all(resp.as_bytes()).await;
                });
            }
        });

        let req = RequestItem::new("Load Target", HttpMethod::Get, format!("http://{addr}/fast"));
        let (tx, mut rx) = mpsc::unbounded_channel();

        let summary = run_load_test(
            req,
            VariableContext::new(),
            AuthConfig::NoAuth,
            LoadTestConfig {
                duration_secs: 1,
                profile: LoadProfile::Constant { vus: 3 },
                settings: AppSettings::default(),
                cancel: Arc::new(AtomicBool::new(false)),
            },
            Some(tx),
        )
        .await;

        assert!(summary.total_requests > 0);
        assert_eq!(summary.error_count, 0);
        assert_eq!(summary.error_rate, 0.0);
        assert!(summary.rps > 0.0);
        assert!(summary.min_ms <= summary.p50_ms);
        assert!(summary.p50_ms <= summary.p95_ms);
        assert!(summary.p95_ms <= summary.p99_ms);
        assert!(summary.p99_ms <= summary.max_ms);
        assert!(!summary.timeline.is_empty());

        // Drain any live progress updates sent during the 1-second test.
        let mut updates = 0;
        while rx.try_recv().is_ok() {
            updates += 1;
        }
        assert!(updates >= 1 || !summary.timeline.is_empty());

        server_handle.abort();
    }
}
