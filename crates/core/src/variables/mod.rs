use std::collections::{HashMap, HashSet};
use std::fmt;

use chrono::Utc;
use rand::distributions::Alphanumeric;
use rand::Rng;
use uuid::Uuid;

use crate::models::{KeyValue, Variable};

/// Maximum byte length allowed during variable interpolation to prevent exponential expansion DoS.
const MAX_RESOLVED_BYTES: usize = 2 * 1024 * 1024; // 2 MiB

/// Variable resolution scopes ordered from lowest to highest precedence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VariableScope {
    Global,
    Collection,
    Environment,
    Data,
    Local,
}

/// Holds variables across all 5 Postman-compatible scopes.
///
/// Precedence (highest to lowest): `Local > Data > Environment > Collection > Global`.
#[derive(Clone, Default)]
pub struct VariableContext {
    pub global: HashMap<String, String>,
    pub collection: HashMap<String, String>,
    pub environment: HashMap<String, String>,
    pub data: HashMap<String, String>,
    pub local: HashMap<String, String>,
    /// Keys that were loaded from `Variable { secret: true }` entries.
    pub secret_keys: HashSet<String>,
}

impl fmt::Debug for VariableContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fn redact_map<'a>(
            m: &'a HashMap<String, String>,
            secrets: &HashSet<String>,
        ) -> HashMap<&'a str, &'a str> {
            m.iter()
                .map(|(k, v)| {
                    let display = if secrets.contains(k) {
                        "[REDACTED]"
                    } else {
                        v.as_str()
                    };
                    (k.as_str(), display)
                })
                .collect()
        }
        f.debug_struct("VariableContext")
            .field("global", &redact_map(&self.global, &self.secret_keys))
            .field("collection", &redact_map(&self.collection, &self.secret_keys))
            .field(
                "environment",
                &redact_map(&self.environment, &self.secret_keys),
            )
            .field("data", &redact_map(&self.data, &self.secret_keys))
            .field("local", &redact_map(&self.local, &self.secret_keys))
            .field("secret_keys", &self.secret_keys)
            .finish()
    }
}

impl VariableContext {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set(&mut self, scope: VariableScope, key: impl Into<String>, value: impl Into<String>) {
        self.scope_map_mut(scope).insert(key.into(), value.into());
    }

    pub fn unset(&mut self, scope: VariableScope, key: &str) -> Option<String> {
        self.scope_map_mut(scope).remove(key)
    }

    /// Loads only `enabled == true` variables into the given scope.
    pub fn load_variables(&mut self, scope: VariableScope, variables: &[Variable]) {
        for var in variables {
            if var.enabled && !var.key.is_empty() {
                if var.secret {
                    self.secret_keys.insert(var.key.clone());
                }
                self.scope_map_mut(scope)
                    .insert(var.key.clone(), var.value.clone());
            }
        }
    }

    /// Returns `true` if `key` is marked as a secret variable.
    pub fn is_secret(&self, key: &str) -> bool {
        self.secret_keys.contains(key)
    }

    /// Returns a clone of this context with all `secret_keys` replaced by `"[REDACTED]"`.
    pub fn redacted_clone(&self) -> Self {
        let redact = |m: &HashMap<String, String>| -> HashMap<String, String> {
            m.iter()
                .map(|(k, v)| {
                    if self.secret_keys.contains(k) {
                        (k.clone(), "[REDACTED]".to_string())
                    } else {
                        (k.clone(), v.clone())
                    }
                })
                .collect()
        };
        Self {
            global: redact(&self.global),
            collection: redact(&self.collection),
            environment: redact(&self.environment),
            data: redact(&self.data),
            local: redact(&self.local),
            secret_keys: self.secret_keys.clone(),
        }
    }

    /// Looks up a static variable by precedence: Local > Data > Environment > Collection > Global.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.local
            .get(key)
            .or_else(|| self.data.get(key))
            .or_else(|| self.environment.get(key))
            .or_else(|| self.collection.get(key))
            .or_else(|| self.global.get(key))
            .map(String::as_str)
    }

    fn scope_map_mut(&mut self, scope: VariableScope) -> &mut HashMap<String, String> {
        match scope {
            VariableScope::Global => &mut self.global,
            VariableScope::Collection => &mut self.collection,
            VariableScope::Environment => &mut self.environment,
            VariableScope::Data => &mut self.data,
            VariableScope::Local => &mut self.local,
        }
    }
}

/// Scoped `{{variable}}` interpolation engine with built-in dynamic variables.
#[derive(Debug, Clone, Default)]
pub struct VariableResolver {
    context: VariableContext,
    max_depth: usize,
}

impl VariableResolver {
    pub fn new(context: VariableContext) -> Self {
        Self {
            context,
            max_depth: 10,
        }
    }

    pub fn context(&self) -> &VariableContext {
        &self.context
    }

    pub fn context_mut(&mut self) -> &mut VariableContext {
        &mut self.context
    }

    /// Looks up a variable key, checking dynamic `$` variables first, then scoped variables.
    pub fn lookup(&self, key: &str) -> Option<String> {
        let trimmed = key.trim();
        if trimmed.starts_with('$') {
            resolve_dynamic(trimmed)
        } else {
            self.context.get(trimmed).map(str::to_string)
        }
    }

    /// Resolves all `{{variable}}` placeholders in `input`, including nested references
    /// up to `max_depth` passes and `MAX_RESOLVED_BYTES` total output size.
    ///
    /// Unknown placeholders are preserved unchanged.
    pub fn resolve(&self, input: &str) -> String {
        let mut current = input.to_string();
        for _ in 0..self.max_depth {
            let (next, changed, limit_hit) = self.resolve_once(&current);
            if !changed || limit_hit {
                return next;
            }
            current = next;
        }
        current
    }

    /// Resolves placeholders while replacing any `secret_keys` variable with `"[REDACTED]"`.
    pub fn resolve_redacted(&self, input: &str) -> String {
        let redacted_resolver = Self::new(self.context.redacted_clone());
        redacted_resolver.resolve(input)
    }

    /// Resolves keys and values for all entries in a `KeyValue` slice.
    pub fn resolve_key_values(&self, items: &[KeyValue]) -> Vec<KeyValue> {
        items
            .iter()
            .map(|kv| KeyValue {
                key: self.resolve(&kv.key),
                value: self.resolve(&kv.value),
                enabled: kv.enabled,
                description: kv.description.clone(),
            })
            .collect()
    }

    fn resolve_once(&self, input: &str) -> (String, bool, bool) {
        let mut out = String::with_capacity(input.len().min(MAX_RESOLVED_BYTES));
        let mut rest = input;
        let mut changed = false;

        while let Some(start) = rest.find("{{") {
            let prefix = &rest[..start];
            if out.len().saturating_add(prefix.len()) > MAX_RESOLVED_BYTES {
                return (out, changed, true);
            }
            out.push_str(prefix);
            let after_open = &rest[start + 2..];

            if let Some(end) = after_open.find("}}") {
                let key = &after_open[..end];
                // Avoid eating nested `{{` before `}}` (e.g. `{{a{{b}}}}`).
                if key.contains("{{") {
                    if out.len().saturating_add(2) > MAX_RESOLVED_BYTES {
                        return (out, changed, true);
                    }
                    out.push_str("{{");
                    rest = after_open;
                    continue;
                }

                if let Some(val) = self.lookup(key) {
                    if out.len().saturating_add(val.len()) > MAX_RESOLVED_BYTES {
                        return (out, changed, true);
                    }
                    out.push_str(&val);
                    changed = true;
                } else {
                    let placeholder_len = 4usize.saturating_add(key.len());
                    if out.len().saturating_add(placeholder_len) > MAX_RESOLVED_BYTES {
                        return (out, changed, true);
                    }
                    out.push_str("{{");
                    out.push_str(key);
                    out.push_str("}}");
                }
                rest = &after_open[end + 2..];
            } else {
                let tail = &rest[start..];
                if out.len().saturating_add(tail.len()) > MAX_RESOLVED_BYTES {
                    return (out, changed, true);
                }
                out.push_str(tail);
                return (out, changed, false);
            }
        }

        if out.len().saturating_add(rest.len()) > MAX_RESOLVED_BYTES {
            return (out, changed, true);
        }
        out.push_str(rest);
        (out, changed, false)
    }
}

fn resolve_dynamic(name: &str) -> Option<String> {
    let mut rng = rand::thread_rng();
    match name {
        "$guid" | "$randomUUID" => Some(Uuid::new_v4().to_string()),
        "$timestamp" => Some(Utc::now().timestamp().to_string()),
        "$isoTimestamp" => Some(Utc::now().to_rfc3339()),
        "$randomInt" => Some(rng.gen_range(0..=1000).to_string()),
        "$randomBoolean" => Some(if rng.gen::<bool>() { "true" } else { "false" }.to_string()),
        "$randomAlphaNumeric" => {
            let ch = rng.sample(Alphanumeric) as char;
            Some(ch.to_string())
        }
        "$randomEmail" => {
            let user: String = (0..8)
                .map(|_| (rng.sample(Alphanumeric) as char).to_ascii_lowercase())
                .collect();
            Some(format!("user_{user}@example.com"))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn precedence_order_local_over_data_over_env_over_collection_over_global() {
        let mut ctx = VariableContext::new();
        ctx.set(VariableScope::Global, "host", "global.local");
        assert_eq!(ctx.get("host"), Some("global.local"));

        ctx.set(VariableScope::Collection, "host", "collection.local");
        assert_eq!(ctx.get("host"), Some("collection.local"));

        ctx.set(VariableScope::Environment, "host", "env.local");
        assert_eq!(ctx.get("host"), Some("env.local"));

        ctx.set(VariableScope::Data, "host", "data.local");
        assert_eq!(ctx.get("host"), Some("data.local"));

        ctx.set(VariableScope::Local, "host", "local.local");
        assert_eq!(ctx.get("host"), Some("local.local"));
    }

    #[test]
    fn disabled_variables_are_ignored() {
        let mut ctx = VariableContext::new();
        let mut disabled = Variable::new("token", "secret");
        disabled.enabled = false;
        ctx.load_variables(
            VariableScope::Environment,
            &[disabled, Variable::new("active", "yes")],
        );

        let resolver = VariableResolver::new(ctx);
        assert_eq!(resolver.resolve("{{token}} {{active}}"), "{{token}} yes");
    }

    #[test]
    fn nested_variables_and_cycle_safety() {
        let mut ctx = VariableContext::new();
        ctx.set(VariableScope::Environment, "scheme", "https");
        ctx.set(VariableScope::Environment, "host", "api.example.com");
        ctx.set(
            VariableScope::Collection,
            "baseUrl",
            "{{scheme}}://{{host}}/v1",
        );
        ctx.set(VariableScope::Local, "a", "{{b}}");
        ctx.set(VariableScope::Local, "b", "{{a}}");

        let resolver = VariableResolver::new(ctx);
        assert_eq!(
            resolver.resolve("{{baseUrl}}/users"),
            "https://api.example.com/v1/users"
        );
        // Must terminate cleanly on cycles.
        let cycled = resolver.resolve("{{a}}");
        assert!(cycled == "{{a}}" || cycled == "{{b}}");
    }

    #[test]
    fn unknown_and_malformed_placeholders_preserved() {
        let resolver = VariableResolver::new(VariableContext::new());
        assert_eq!(
            resolver.resolve("hello {{missing}} {{unclosed"),
            "hello {{missing}} {{unclosed"
        );
    }

    #[test]
    fn dynamic_variables_resolve_valid_formats() {
        let resolver = VariableResolver::new(VariableContext::new());

        let guid = resolver.resolve("{{$guid}}");
        assert!(Uuid::parse_str(&guid).is_ok());

        let uuid = resolver.resolve("{{$randomUUID}}");
        assert!(Uuid::parse_str(&uuid).is_ok());

        let ts: i64 = resolver.resolve("{{$timestamp}}").parse().expect("unix ts");
        assert!(ts > 1_700_000_000);

        let iso = resolver.resolve("{{$isoTimestamp}}");
        assert!(chrono::DateTime::parse_from_rfc3339(&iso).is_ok());

        let num: u32 = resolver
            .resolve("{{$randomInt}}")
            .parse()
            .expect("randomInt");
        assert!(num <= 1000);

        let email = resolver.resolve("{{$randomEmail}}");
        assert!(email.starts_with("user_") && email.ends_with("@example.com"));

        let boolean = resolver.resolve("{{$randomBoolean}}");
        assert!(boolean == "true" || boolean == "false");

        let alnum = resolver.resolve("{{$randomAlphaNumeric}}");
        assert_eq!(alnum.len(), 1);
        assert!(alnum.chars().all(|c| c.is_ascii_alphanumeric()));
    }

    #[test]
    fn exponential_variable_expansion_is_bounded_and_secrets_redacted() {
        let mut ctx = VariableContext::new();
        ctx.load_variables(
            VariableScope::Environment,
            &[
                Variable::secret("apiKey", "super_secret_value"),
                Variable::new("a", "{{b}}{{b}}{{b}}{{b}}{{b}}{{b}}{{b}}{{b}}"),
                Variable::new("b", "{{c}}{{c}}{{c}}{{c}}{{c}}{{c}}{{c}}{{c}}"),
                Variable::new("c", "{{d}}{{d}}{{d}}{{d}}{{d}}{{d}}{{d}}{{d}}"),
                Variable::new("d", "0123456789abcdef0123456789abcdef"),
            ],
        );

        let dbg = format!("{ctx:?}");
        assert!(!dbg.contains("super_secret_value"));
        assert!(dbg.contains("[REDACTED]"));

        let resolver = VariableResolver::new(ctx);
        assert_eq!(
            resolver.resolve_redacted("https://api.test/?k={{apiKey}}"),
            "https://api.test/?k=[REDACTED]"
        );

        let expanded = resolver.resolve("{{a}}{{a}}{{a}}{{a}}");
        assert!(expanded.len() <= MAX_RESOLVED_BYTES);
    }
}
