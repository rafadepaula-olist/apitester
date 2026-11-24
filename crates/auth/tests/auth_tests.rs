use std::time::Duration;

use apitester_auth::{
    apply_auth, apply_digest_challenge, apply_ntlm_challenge, build_authorization_url_with_pkce,
    build_type1_negotiate, build_type3_authenticate, exchange_client_credentials,
    exchange_password, listen_oauth2_callback_on_listener, refresh_access_token, ApiKeyLocation,
    AsapConfig, AuthConfig, AuthRequestContext, AwsV4Config, DigestAuthConfig, EdgeGridConfig,
    HawkConfig, JwtConfig, NtlmConfig, OAuth1Config, OAuth2Config,
};
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use base64::Engine;
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

fn sample_ctx() -> AuthRequestContext {
    AuthRequestContext {
        method: "GET".into(),
        url: "https://api.example.com/v1/items?b=2&a=1".into(),
        headers: vec![("Content-Type".into(), "application/json".into())],
        body: Vec::new(),
    }
}

#[test]
fn no_auth_and_inherit_return_empty() {
    let ctx = sample_ctx();
    let r1 = apply_auth(&AuthConfig::NoAuth, &ctx).unwrap();
    assert!(r1.headers.is_empty() && r1.query_params.is_empty());

    let r2 = apply_auth(&AuthConfig::Inherit, &ctx).unwrap();
    assert!(r2.headers.is_empty() && r2.query_params.is_empty());
}

#[test]
fn bearer_auth_sets_authorization_header() {
    let ctx = sample_ctx();
    let app = apply_auth(
        &AuthConfig::Bearer {
            token: "my-secret-token".into(),
        },
        &ctx,
    )
    .unwrap();
    assert_eq!(
        app.headers,
        vec![("Authorization".into(), "Bearer my-secret-token".into())]
    );
}

#[test]
fn basic_auth_encodes_credentials() {
    let ctx = sample_ctx();
    let app = apply_auth(
        &AuthConfig::Basic {
            username: "Aladdin".into(),
            password: "open sesame".into(),
        },
        &ctx,
    )
    .unwrap();
    assert_eq!(
        app.headers,
        vec![(
            "Authorization".into(),
            "Basic QWxhZGRpbjpvcGVuIHNlc2FtZQ==".into()
        )]
    );
}

#[test]
fn api_key_header_and_query() {
    let ctx = sample_ctx();
    let h = apply_auth(
        &AuthConfig::ApiKey {
            key: "X-API-Key".into(),
            value: "abc123".into(),
            add_to: ApiKeyLocation::Header,
        },
        &ctx,
    )
    .unwrap();
    assert_eq!(h.headers, vec![("X-API-Key".into(), "abc123".into())]);
    assert!(h.query_params.is_empty());

    let q = apply_auth(
        &AuthConfig::ApiKey {
            key: "api_key".into(),
            value: "xyz789".into(),
            add_to: ApiKeyLocation::Query,
        },
        &ctx,
    )
    .unwrap();
    assert!(q.headers.is_empty());
    assert_eq!(q.query_params, vec![("api_key".into(), "xyz789".into())]);
}

#[test]
fn oauth2_header_and_query_placement() {
    let ctx = sample_ctx();
    let cfg = OAuth2Config {
        access_token: "tok_123".into(),
        header_prefix: "Token".into(),
        add_token_to: ApiKeyLocation::Header,
        ..Default::default()
    };
    let h = apply_auth(&AuthConfig::OAuth2(cfg), &ctx).unwrap();
    assert_eq!(
        h.headers,
        vec![("Authorization".into(), "Token tok_123".into())]
    );

    let cfg_q = OAuth2Config {
        access_token: "tok_456".into(),
        add_token_to: ApiKeyLocation::Query,
        ..Default::default()
    };
    let q = apply_auth(&AuthConfig::OAuth2(cfg_q), &ctx).unwrap();
    assert_eq!(
        q.query_params,
        vec![("access_token".into(), "tok_456".into())]
    );
}

#[test]
fn oauth2_pkce_url_generation_is_valid_s256() {
    let cfg = OAuth2Config {
        auth_url: "https://auth.example.com/authorize".into(),
        client_id: "client-1".into(),
        redirect_uri: "http://127.0.0.1:8765/callback".into(),
        scope: "read write".into(),
        state: "fixed-state".into(),
        ..Default::default()
    };
    let req = build_authorization_url_with_pkce(&cfg).unwrap();
    assert_eq!(req.state, "fixed-state");
    assert_eq!(req.pkce.method, "S256");

    // Verify S256 challenge = base64url(sha256(verifier))
    let mut hasher = Sha256::new();
    hasher.update(req.pkce.verifier.as_bytes());
    let expected = URL_SAFE_NO_PAD.encode(hasher.finalize());
    assert_eq!(req.pkce.challenge, expected);

    let parsed = url::Url::parse(&req.url).unwrap();
    let params: std::collections::HashMap<_, _> = parsed.query_pairs().into_owned().collect();
    assert_eq!(params.get("response_type").map(String::as_str), Some("code"));
    assert_eq!(params.get("client_id").map(String::as_str), Some("client-1"));
    assert_eq!(
        params.get("code_challenge").map(String::as_str),
        Some(expected.as_str())
    );
    assert_eq!(
        params.get("code_challenge_method").map(String::as_str),
        Some("S256")
    );
}

#[tokio::test]
async fn oauth2_callback_listener_captures_code_and_state() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let server = tokio::spawn(async move {
        listen_oauth2_callback_on_listener(listener, Duration::from_secs(5)).await
    });

    let mut client = tokio::net::TcpStream::connect(addr).await.unwrap();
    client
        .write_all(b"GET /callback?code=abc_code&state=xyz_state HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n")
        .await
        .unwrap();

    let mut resp = Vec::new();
    let _ = client.read_to_end(&mut resp).await;
    let resp_str = String::from_utf8_lossy(&resp);
    assert!(resp_str.starts_with("HTTP/1.1 200 OK"));

    let cb = server.await.unwrap().unwrap();
    assert_eq!(cb.code.as_deref(), Some("abc_code"));
    assert_eq!(cb.state.as_deref(), Some("xyz_state"));
    assert!(cb.error.is_none());
}

#[tokio::test]
async fn oauth2_token_exchange_helpers_hit_token_endpoint() {
    async fn spawn_mock_token_server(expected_grant: &'static str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 4096];
            let n = stream.read(&mut buf).await.unwrap();
            let req = String::from_utf8_lossy(&buf[..n]);
            assert!(
                req.contains(&format!("grant_type={expected_grant}")),
                "unexpected request body: {req}"
            );
            let body = r#"{"access_token":"new_tok","token_type":"Bearer","expires_in":3600,"refresh_token":"ref_2"}"#;
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(resp.as_bytes()).await.unwrap();
        });
        format!("http://{addr}/token")
    }

    let cc_url = spawn_mock_token_server("client_credentials").await;
    let cc_cfg = OAuth2Config {
        token_url: cc_url,
        client_id: "cid".into(),
        client_secret: "csec".into(),
        scope: "api".into(),
        ..Default::default()
    };
    let tok = exchange_client_credentials(&cc_cfg).await.unwrap();
    assert_eq!(tok.access_token, "new_tok");
    assert_eq!(tok.expires_in, Some(3600));

    let pw_url = spawn_mock_token_server("password").await;
    let pw_cfg = OAuth2Config {
        token_url: pw_url,
        client_id: "cid".into(),
        ..Default::default()
    };
    let tok2 = exchange_password(&pw_cfg, "alice", "secret").await.unwrap();
    assert_eq!(tok2.access_token, "new_tok");

    let ref_url = spawn_mock_token_server("refresh_token").await;
    let ref_cfg = OAuth2Config {
        token_url: ref_url,
        client_id: "cid".into(),
        refresh_token: Some("ref_1".into()),
        ..Default::default()
    };
    let tok3 = refresh_access_token(&ref_cfg).await.unwrap();
    assert_eq!(tok3.refresh_token.as_deref(), Some("ref_2"));
}

#[test]
fn aws_v4_header_with_session_token_and_query_mode() {
    let ctx = sample_ctx();
    let cfg = AwsV4Config {
        access_key: "AKIAIOSFODNN7EXAMPLE".into(),
        secret_key: "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY".into(),
        region: "us-west-2".into(),
        service: "s3".into(),
        session_token: Some("sess-tok-123".into()),
        add_to: ApiKeyLocation::Header,
    };
    let app = apply_auth(&AuthConfig::AwsV4(cfg.clone()), &ctx).unwrap();
    assert!(app.headers.iter().any(|(k, _)| k == "X-Amz-Date"));
    assert!(app
        .headers
        .iter()
        .any(|(k, v)| k == "X-Amz-Security-Token" && v == "sess-tok-123"));
    let auth = &app
        .headers
        .iter()
        .find(|(k, _)| k == "Authorization")
        .unwrap()
        .1;
    assert!(auth.starts_with("AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/"));
    assert!(auth.contains("x-amz-security-token"));
    assert!(auth.contains("Signature="));

    let mut q_cfg = cfg;
    q_cfg.add_to = ApiKeyLocation::Query;
    let q_app = apply_auth(&AuthConfig::AwsV4(q_cfg), &ctx).unwrap();
    assert!(q_app.headers.is_empty());
    assert!(q_app
        .query_params
        .iter()
        .any(|(k, v)| k == "X-Amz-Algorithm" && v == "AWS4-HMAC-SHA256"));
    assert!(q_app
        .query_params
        .iter()
        .any(|(k, _)| k == "X-Amz-Signature"));
}

#[test]
fn jwt_hs256_hs384_hs512_and_query_placement() {
    let ctx = sample_ctx();
    // Known JWT.io HS256 vector: header {"alg":"HS256","typ":"JWT"}, payload {"sub":"1234567890","name":"John Doe","iat":1516239022}, secret "your-256-bit-secret"
    for alg in ["HS256", "HS384", "HS512"] {
        let cfg = JwtConfig {
            algorithm: alg.into(),
            secret: "super-secret-key".into(),
            is_secret_base64: false,
            payload: r#"{"sub":"42","role":"admin"}"#.into(),
            header_prefix: "Bearer".into(),
            query_param_key: "jwt".into(),
            add_to: ApiKeyLocation::Header,
            custom_headers: Some(r#"{"kid":"k1"}"#.into()),
        };
        let app = apply_auth(&AuthConfig::Jwt(cfg), &ctx).unwrap();
        let val = &app.headers[0].1;
        let token = val.strip_prefix("Bearer ").unwrap();
        let parts: Vec<&str> = token.split('.').collect();
        assert_eq!(parts.len(), 3);

        let header: Value =
            serde_json::from_slice(&URL_SAFE_NO_PAD.decode(parts[0]).unwrap()).unwrap();
        assert_eq!(header["alg"], alg);
        assert_eq!(header["typ"], "JWT");
        assert_eq!(header["kid"], "k1");

        let payload: Value =
            serde_json::from_slice(&URL_SAFE_NO_PAD.decode(parts[1]).unwrap()).unwrap();
        assert_eq!(payload["sub"], "42");
        assert_eq!(payload["role"], "admin");
    }

    let q_cfg = JwtConfig {
        secret: STANDARD.encode("raw-secret"),
        is_secret_base64: true,
        add_to: ApiKeyLocation::Query,
        query_param_key: "jwt_tok".into(),
        ..Default::default()
    };
    let q_app = apply_auth(&AuthConfig::Jwt(q_cfg), &ctx).unwrap();
    assert_eq!(q_app.query_params[0].0, "jwt_tok");
    assert_eq!(q_app.query_params[0].1.split('.').count(), 3);
}

#[test]
fn digest_rfc7616_md5_and_sha256_challenge() {
    // RFC 7616 Section 3.9.1 MD5 and SHA-256 test vectors
    let cfg = DigestAuthConfig {
        username: "Mufasa".into(),
        password: "Circle of Life".into(),
        nonce_count: "00000001".into(),
        client_nonce: "f2/wE4q74E6zIJEtWaHKaf5wv/H5QzzpXusqGemxURZJ".into(),
        ..Default::default()
    };
    let ctx = AuthRequestContext {
        method: "GET".into(),
        url: "http://www.example.org/dir/index.html".into(),
        headers: Vec::new(),
        body: Vec::new(),
    };
    let challenge = r#"Digest realm="http-auth@example.org", qop="auth, auth-int", algorithm=MD5, nonce="7ypf/xlj9XXwfDPEoM4URrv/xwf94BcCAzFZH4GiTo0v", opaque="FQhe/qaU925kfnzjCev0ciny7QMkPqMAFRtzCUYo5tdS""#;

    let app = apply_digest_challenge(&cfg, &ctx, challenge).unwrap();
    let header = &app.headers[0].1;
    assert!(header.starts_with("Digest "));
    assert!(header.contains(r#"username="Mufasa""#));
    assert!(header.contains(r#"realm="http-auth@example.org""#));
    assert!(header.contains(r#"uri="/dir/index.html""#));
    assert!(header.contains(r#"response="8ca523f5e9506fed4657c9700eebdbec""#));
    assert!(header.contains("qop=auth"));
    assert!(header.contains("nc=00000001"));

    // RFC 7616 Section 3.9.1 SHA-256 test vector
    let challenge_sha256 = r#"Digest realm="http-auth@example.org", qop="auth, auth-int", algorithm=SHA-256, nonce="7ypf/xlj9XXwfDPEoM4URrv/xwf94BcCAzFZH4GiTo0v", opaque="FQhe/qaU925kfnzjCev0ciny7QMkPqMAFRtzCUYo5tdS""#;
    let app2 = apply_digest_challenge(&cfg, &ctx, challenge_sha256).unwrap();
    let header2 = &app2.headers[0].1;
    assert!(header2.contains("algorithm=SHA-256"));
    assert!(header2.contains(
        r#"response="753927fa0e85d155564e2e272a28d1802ca10daf4496794697cf8db5856cb6c1""#
    ));

    // Non-ASCII challenge must return InvalidChallenge without panicking on UTF-8 boundary
    let non_ascii = apply_digest_challenge(&cfg, &ctx, "éééé realm=\"x\"");
    assert!(non_ascii.is_err());
}

#[test]
fn oauth1_rfc5849_hmac_sha1_and_hmac_sha256() {
    // RFC 5849 Section 1.2 / 3.4.2 test vector
    let cfg = OAuth1Config {
        consumer_key: "dpf43f3p2l4k3l03".into(),
        consumer_secret: "kd94hf93k423kf44".into(),
        token: "nnch734d00sl2jdk".into(),
        token_secret: "pfkkdhi9sl3r4s00".into(),
        signature_method: "HMAC-SHA1".into(),
        timestamp: Some("1191242096".into()),
        nonce: Some("kllo9940pd9333jh".into()),
        version: "1.0".into(),
        realm: None,
        add_params_to_header: true,
        add_empty_params_to_sign: false,
    };
    let ctx = AuthRequestContext {
        method: "GET".into(),
        url: "http://photos.example.net/photos?file=vacation.jpg&size=original".into(),
        headers: Vec::new(),
        body: Vec::new(),
    };
    let app = apply_auth(&AuthConfig::OAuth1(cfg.clone()), &ctx).unwrap();
    let header = &app.headers[0].1;
    assert!(header.starts_with("OAuth "));
    // Expected signature: tR3+Ty81lMeYAr/Fid0kMTYa/WM= -> percent-encoded: tR3%2BTy81lMeYAr%2FFid0kMTYa%2FWM%3D
    assert!(
        header.contains(r#"oauth_signature="tR3%2BTy81lMeYAr%2FFid0kMTYa%2FWM%3D""#),
        "got header: {header}"
    );

    let mut cfg256 = cfg;
    cfg256.signature_method = "HMAC-SHA256".into();
    cfg256.add_params_to_header = false;
    let app256 = apply_auth(&AuthConfig::OAuth1(cfg256), &ctx).unwrap();
    assert!(app256.headers.is_empty());
    assert!(app256
        .query_params
        .iter()
        .any(|(k, v)| k == "oauth_signature_method" && v == "HMAC-SHA256"));
    assert!(app256
        .query_params
        .iter()
        .any(|(k, v)| k == "oauth_signature" && !v.is_empty()));
}

#[test]
fn hawk_header_matches_spec_vector_and_payload_hash() {
    // Official Hawk README test vector (without payload hash)
    let cfg = HawkConfig {
        auth_id: "dh37fgj492je".into(),
        auth_key: "werxhqb98rpaxn39848xrunpaw3489ruxnpa98w4rxn".into(),
        algorithm: "sha256".into(),
        nonce: "j4h3g2".into(),
        extra_data: "some-app-ext-data".into(),
        timestamp: Some("1353832234".into()),
        include_payload_hash: false,
        ..Default::default()
    };
    let ctx = AuthRequestContext {
        method: "GET".into(),
        url: "http://example.com:8000/resource/1?b=1&a=2".into(),
        headers: Vec::new(),
        body: Vec::new(),
    };
    let app = apply_auth(&AuthConfig::Hawk(cfg.clone()), &ctx).unwrap();
    let header = &app.headers[0].1;
    assert_eq!(
        header,
        r#"Hawk id="dh37fgj492je", ts="1353832234", nonce="j4h3g2", ext="some-app-ext-data", mac="6R4rV5iE+NPoym+WwjeHzjAGXUtLNIxmo1vpMofpLAE=""#
    );

    // With payload hash (official Hawk README POST vector)
    let mut cfg_post = cfg;
    cfg_post.include_payload_hash = true;
    let ctx_post = AuthRequestContext {
        method: "POST".into(),
        url: "http://example.com:8000/resource/1?b=1&a=2".into(),
        headers: vec![("Content-Type".into(), "text/plain; charset=utf-8".into())],
        body: b"Thank you for flying Hawk".to_vec(),
    };
    let app_post = apply_auth(&AuthConfig::Hawk(cfg_post), &ctx_post).unwrap();
    let header_post = &app_post.headers[0].1;
    assert!(
        header_post.contains(r#"hash="Yi9LfIIFRtBEPt74PVmbTF/xVAwPn7ub15ePICfgnuY=""#),
        "got header: {header_post}"
    );
    assert!(
        header_post.contains(r#"mac="aSe1DERmZuRl3pI36/9BdZmnErTw3sNzOOAUlfeKjVw=""#),
        "got header: {header_post}"
    );
}

#[test]
fn edgegrid_formats_eg1_hmac_sha256_header() {
    let cfg = EdgeGridConfig {
        access_token: "akab-access-token".into(),
        client_token: "akab-client-token".into(),
        client_secret: "akab-client-secret".into(),
        nonce: "nonce-1234".into(),
        timestamp: Some("20240101T00:00:00+0000".into()),
        base_url: "https://akaa-example.luna.akamaiapis.net".into(),
        headers_to_sign: "X-Custom".into(),
    };
    let ctx = AuthRequestContext {
        method: "POST".into(),
        url: "https://akaa-example.luna.akamaiapis.net/ccu/v3/invalidate/url".into(),
        headers: vec![("X-Custom".into(), "  hello   world  ".into())],
        body: br#"{"objects":["/index.html"]}"#.to_vec(),
    };
    let app = apply_auth(&AuthConfig::EdgeGrid(cfg), &ctx).unwrap();
    let header = &app.headers[0].1;
    assert!(header.starts_with(
        "EG1-HMAC-SHA256 client_token=akab-client-token;access_token=akab-access-token;timestamp=20240101T00:00:00+0000;nonce=nonce-1234;signature="
    ));
    let sig = header.split("signature=").nth(1).unwrap();
    assert_eq!(STANDARD.decode(sig).unwrap().len(), 32);
}

#[test]
fn asap_generates_signed_jwt_bearer_header() {
    let cfg = AsapConfig {
        alg: "HS256".into(),
        issuer: "service-a".into(),
        kid: "service-a/key-1".into(),
        audience: "service-b".into(),
        subject: "user-99".into(),
        private_key: "asap-shared-test-secret".into(),
        claims: r#"{"scope":"admin"}"#.into(),
        expiry: 120,
    };
    let ctx = sample_ctx();
    let app = apply_auth(&AuthConfig::Asap(cfg), &ctx).unwrap();
    let token = app.headers[0].1.strip_prefix("Bearer ").unwrap();
    let parts: Vec<&str> = token.split('.').collect();
    assert_eq!(parts.len(), 3);

    let header: Value =
        serde_json::from_slice(&URL_SAFE_NO_PAD.decode(parts[0]).unwrap()).unwrap();
    assert_eq!(header["alg"], "HS256");
    assert_eq!(header["kid"], "service-a/key-1");

    let claims: Value =
        serde_json::from_slice(&URL_SAFE_NO_PAD.decode(parts[1]).unwrap()).unwrap();
    assert_eq!(claims["iss"], "service-a");
    assert_eq!(claims["aud"], "service-b");
    assert_eq!(claims["sub"], "user-99");
    assert_eq!(claims["scope"], "admin");
    assert!(claims["jti"].is_string());
    assert_eq!(
        claims["exp"].as_i64().unwrap() - claims["iat"].as_i64().unwrap(),
        120
    );
}

#[test]
fn ntlm_type1_and_type3_handshake() {
    let cfg = NtlmConfig {
        username: "jdoe".into(),
        password: "Password123!".into(),
        domain: "CORP".into(),
        workstation: "WORKSTATION1".into(),
    };
    let ctx = sample_ctx();

    // Type 1 via apply_auth
    let app1 = apply_auth(&AuthConfig::Ntlm(cfg.clone()), &ctx).unwrap();
    let type1_b64 = app1.headers[0].1.strip_prefix("NTLM ").unwrap();
    assert_eq!(type1_b64, build_type1_negotiate(&cfg).unwrap());
    let type1_bytes = STANDARD.decode(type1_b64).unwrap();
    assert_eq!(&type1_bytes[..8], b"NTLMSSP\0");
    assert_eq!(
        u32::from_le_bytes(type1_bytes[8..12].try_into().unwrap()),
        1
    );

    // Construct a synthetic NTLMSSP Type 2 Challenge
    let mut type2 = Vec::new();
    type2.extend_from_slice(b"NTLMSSP\0");
    type2.extend_from_slice(&2u32.to_le_bytes()); // Type 2
    type2.extend_from_slice(&[0u8; 8]); // TargetName secbuf
    type2.extend_from_slice(&0x0008_8201u32.to_le_bytes()); // Flags
    type2.extend_from_slice(&[0x01, 0x23, 0x45, 0x67, 0x89, 0xAB, 0xCD, 0xEF]); // ServerChallenge
    type2.extend_from_slice(&[0u8; 8]); // Reserved
    type2.extend_from_slice(&[0u8; 8]); // TargetInfo secbuf
    let type2_b64 = STANDARD.encode(&type2);

    // Type 3 via build_type3_authenticate & apply_ntlm_challenge
    let type3_b64 = build_type3_authenticate(&cfg, &type2_b64).unwrap();
    let type3_bytes = STANDARD.decode(&type3_b64).unwrap();
    assert_eq!(&type3_bytes[..8], b"NTLMSSP\0");
    assert_eq!(
        u32::from_le_bytes(type3_bytes[8..12].try_into().unwrap()),
        3
    );

    let app3 = apply_ntlm_challenge(&cfg, &format!("NTLM {type2_b64}")).unwrap();
    assert!(app3.headers[0].1.starts_with("NTLM "));
}
