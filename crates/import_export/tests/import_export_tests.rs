use std::path::Path;

use apitester_core::models::{
    ApiKeyLocation, AsapConfig, AuthConfig, AwsV4Config, Collection, CollectionNode, DigestConfig,
    EdgeGridConfig, Environment, Folder, FormField, FormFieldKind, HawkConfig, HttpMethod,
    JwtConfig, KeyValue, NtlmConfig, OAuth1Config, OAuth2Config, OAuth2GrantType, RawLanguage,
    RequestBody, RequestItem, ResponseData, SavedExample, ScriptConfig, Variable,
};
use apitester_core::variables::{VariableContext, VariableScope};
use apitester_import_export::{
    export_curl, export_postman_collection, export_postman_environment, generate_code, import_curl,
    import_har, import_openapi, import_postman_collection, import_postman_environment,
    CodegenLanguage, ImportExportError,
};
use uuid::Uuid;

// ============================================================================
// Postman Collection v2.1.0 & Environment Tests
// ============================================================================

#[test]
fn postman_collection_full_round_trip_all_features() {
    let mut col = Collection::new("Synthetic API Suite");
    col.description = "Full-featured collection".into();
    col.auth = AuthConfig::Bearer {
        token: "{{apiToken}}".into(),
    };
    col.scripts = ScriptConfig {
        pre_request: "console.log('col pre');".into(),
        post_response: "pm.test('col ok', () => {});".into(),
    };
    col.variables = vec![
        Variable::new("baseUrl", "https://api.example.com"),
        Variable::secret("apiToken", "tok_test_123"),
        Variable {
            key: "disabledVar".into(),
            value: "ignored".into(),
            enabled: false,
            secret: false,
        },
    ];

    // Request 1: JSON body + query params + headers + scripts + saved example
    let mut r_json = RequestItem::new("Create User", HttpMethod::Post, "{{baseUrl}}/users");
    r_json.params.push(KeyValue::new("notify", "true"));
    r_json.params.push(KeyValue {
        key: "debug".into(),
        value: "1".into(),
        enabled: false,
        description: "optional debug flag".into(),
    });
    r_json
        .headers
        .push(KeyValue::new("Content-Type", "application/json"));
    r_json.body = RequestBody::Raw {
        content: r#"{"name":"Ada"}"#.into(),
        language: RawLanguage::Json,
    };
    r_json.scripts = ScriptConfig {
        pre_request: "pm.variables.set('ts', Date.now());".into(),
        post_response: "pm.response.to.have.status(201);".into(),
    };
    r_json.examples.push(SavedExample {
        id: Uuid::new_v4(),
        name: "201 Created".into(),
        request_url: "https://api.example.com/users?notify=true".into(),
        request_method: HttpMethod::Post,
        request_headers: vec![KeyValue::new("Content-Type", "application/json")],
        request_body: RequestBody::Raw {
            content: r#"{"name":"Ada"}"#.into(),
            language: RawLanguage::Json,
        },
        response: ResponseData {
            status: 201,
            status_text: "Created".into(),
            headers: vec![KeyValue::new("Content-Type", "application/json")],
            body: br#"{"id":42}"#.to_vec(),
            size_bytes: 9,
            ..ResponseData::default()
        },
    });

    // Request 2: UrlEncoded
    let mut r_urlenc = RequestItem::new("Login", HttpMethod::Post, "{{baseUrl}}/login");
    r_urlenc.body = RequestBody::UrlEncoded(vec![
        KeyValue::new("user", "ada"),
        KeyValue::new("pass", "secret"),
    ]);

    // Request 3: FormData (text + file)
    let mut r_form = RequestItem::new("Upload", HttpMethod::Post, "{{baseUrl}}/upload");
    r_form.body = RequestBody::FormData(vec![
        FormField::text("title", "avatar"),
        FormField {
            key: "file".into(),
            kind: FormFieldKind::File {
                path: "/tmp/avatar.png".into(),
                content_type: Some("image/png".into()),
            },
            enabled: true,
            description: "profile image".into(),
        },
    ]);

    // Request 4: Binary
    let mut r_bin = RequestItem::new("Upload Raw", HttpMethod::Put, "{{baseUrl}}/blob");
    r_bin.body = RequestBody::Binary {
        path: "/tmp/data.bin".into(),
    };

    // Request 5: GraphQL
    let mut r_gql = RequestItem::new("Get User GQL", HttpMethod::Post, "{{baseUrl}}/graphql");
    r_gql.body = RequestBody::GraphQL {
        query: "query ($id: ID!) { user(id: $id) { name } }".into(),
        variables: r#"{"id":"1"}"#.into(),
    };

    // Nested folder structure
    let mut inner_folder = Folder::new("Uploads");
    inner_folder.auth = AuthConfig::ApiKey {
        key: "X-Upload-Key".into(),
        value: "up_123".into(),
        add_to: ApiKeyLocation::Header,
    };
    inner_folder.items = vec![
        CollectionNode::Request(r_form),
        CollectionNode::Request(r_bin),
    ];

    let mut outer_folder = Folder::new("Users");
    outer_folder.description = "User management endpoints".into();
    outer_folder.scripts.pre_request = "console.log('folder pre');".into();
    outer_folder.items = vec![
        CollectionNode::Request(r_json),
        CollectionNode::Request(r_urlenc),
        CollectionNode::Folder(inner_folder),
    ];

    col.items = vec![
        CollectionNode::Folder(outer_folder),
        CollectionNode::Request(r_gql),
    ];

    let exported = export_postman_collection(&col).expect("export postman");
    let imported = import_postman_collection(&exported).expect("import postman");

    assert_eq!(imported.id, col.id);
    assert_eq!(imported.name, "Synthetic API Suite");
    assert_eq!(imported.description, "Full-featured collection");
    assert_eq!(imported.auth, col.auth);
    assert_eq!(imported.scripts, col.scripts);
    assert_eq!(imported.variables, col.variables);
    assert_eq!(imported.items.len(), 2);

    // Check nested folder
    let CollectionNode::Folder(f_outer) = &imported.items[0] else {
        panic!("expected outer folder");
    };
    assert_eq!(f_outer.name, "Users");
    assert_eq!(f_outer.description, "User management endpoints");
    assert_eq!(f_outer.scripts.pre_request, "console.log('folder pre');");
    assert_eq!(f_outer.items.len(), 3);

    let CollectionNode::Request(imp_json) = &f_outer.items[0] else {
        panic!("expected Create User request");
    };
    assert_eq!(imp_json.method, HttpMethod::Post);
    assert_eq!(imp_json.url, "{{baseUrl}}/users");
    assert_eq!(imp_json.params.len(), 2);
    assert!(imp_json.params[0].enabled);
    assert!(!imp_json.params[1].enabled);
    assert_eq!(
        imp_json.body,
        RequestBody::Raw {
            content: r#"{"name":"Ada"}"#.into(),
            language: RawLanguage::Json,
        }
    );
    assert_eq!(imp_json.examples.len(), 1);
    assert_eq!(imp_json.examples[0].response.status, 201);
    assert_eq!(imp_json.examples[0].response.body_text(), r#"{"id":42}"#);

    let CollectionNode::Folder(f_inner) = &f_outer.items[2] else {
        panic!("expected inner Uploads folder");
    };
    assert_eq!(f_inner.name, "Uploads");
    assert_eq!(
        f_inner.auth,
        AuthConfig::ApiKey {
            key: "X-Upload-Key".into(),
            value: "up_123".into(),
            add_to: ApiKeyLocation::Header,
        }
    );

    let CollectionNode::Request(imp_gql) = &imported.items[1] else {
        panic!("expected GraphQL request");
    };
    assert!(matches!(imp_gql.body, RequestBody::GraphQL { .. }));
}

#[test]
fn postman_all_13_auth_types_round_trip() {
    let auth_cases: Vec<AuthConfig> = vec![
        AuthConfig::NoAuth,
        AuthConfig::Bearer {
            token: "tok123".into(),
        },
        AuthConfig::Basic {
            username: "alice".into(),
            password: "pw".into(),
        },
        AuthConfig::ApiKey {
            key: "api_key".into(),
            value: "k999".into(),
            add_to: ApiKeyLocation::Query,
        },
        AuthConfig::OAuth2(OAuth2Config {
            grant_type: OAuth2GrantType::ClientCredentials,
            access_token: "at_1".into(),
            refresh_token: Some("rt_1".into()),
            token_type: "bearer".into(),
            header_prefix: "Bearer".into(),
            auth_url: "https://auth.example.com/authorize".into(),
            token_url: "https://auth.example.com/token".into(),
            redirect_uri: "https://app.example.com/cb".into(),
            client_id: "cid".into(),
            client_secret: "csec".into(),
            scope: "read write".into(),
            state: "xyz".into(),
            add_token_to: ApiKeyLocation::Header,
            expires_at: None,
        }),
        AuthConfig::AwsV4(AwsV4Config {
            access_key: "AKIA_TEST".into(),
            secret_key: "SECRET_TEST".into(),
            region: "us-east-1".into(),
            service: "s3".into(),
            session_token: Some("sess".into()),
            add_to: ApiKeyLocation::Query,
        }),
        AuthConfig::Jwt(JwtConfig {
            algorithm: "HS512".into(),
            secret: "mysecret".into(),
            is_secret_base64: true,
            payload: r#"{"sub":"123"}"#.into(),
            header_prefix: "Bearer".into(),
            query_param_key: "jwt".into(),
            add_to: ApiKeyLocation::Header,
            custom_headers: Some(r#"{"kid":"k1"}"#.into()),
        }),
        AuthConfig::Digest(DigestConfig {
            username: "u".into(),
            password: "p".into(),
            realm: "r".into(),
            nonce: "n".into(),
            algorithm: "MD5".into(),
            qop: "auth".into(),
            nonce_count: "00000001".into(),
            client_nonce: "cn".into(),
            opaque: "op".into(),
        }),
        AuthConfig::OAuth1(OAuth1Config {
            consumer_key: "ck".into(),
            consumer_secret: "cs".into(),
            token: "t".into(),
            token_secret: "ts".into(),
            signature_method: "HMAC-SHA256".into(),
            timestamp: Some("1700000000".into()),
            nonce: Some("abc".into()),
            version: "1.0".into(),
            realm: Some("realm1".into()),
            add_params_to_header: true,
            add_empty_params_to_sign: true,
        }),
        AuthConfig::Ntlm(NtlmConfig {
            username: "u".into(),
            password: "p".into(),
            domain: "CORP".into(),
            workstation: "WS01".into(),
        }),
        AuthConfig::Hawk(HawkConfig {
            auth_id: "hid".into(),
            auth_key: "hkey".into(),
            algorithm: "sha256".into(),
            user: "hu".into(),
            nonce: "hn".into(),
            extra_data: "ext".into(),
            app: "app".into(),
            delegation: "dlg".into(),
            timestamp: Some("1700000001".into()),
            include_payload_hash: true,
        }),
        AuthConfig::EdgeGrid(EdgeGridConfig {
            access_token: "eat".into(),
            client_token: "ect".into(),
            client_secret: "ecs".into(),
            nonce: "en".into(),
            timestamp: Some("20260101T00:00:00+0000".into()),
            base_url: "https://akab.luna.akamaiapis.net".into(),
            headers_to_sign: "X-Custom".into(),
        }),
        AuthConfig::Asap(AsapConfig {
            alg: "RS256".into(),
            issuer: "iss1".into(),
            kid: "kid1".into(),
            audience: "aud1".into(),
            subject: "sub1".into(),
            private_key: "PEM_KEY".into(),
            claims: r#"{"role":"admin"}"#.into(),
            expiry: 120,
        }),
    ];

    let mut col = Collection::new("All Auth Types");
    for (i, auth) in auth_cases.iter().enumerate() {
        let mut req = RequestItem::new(format!("Req {i}"), HttpMethod::Get, "https://example.com");
        req.auth = auth.clone();
        col.items.push(CollectionNode::Request(req));
    }

    let exported = export_postman_collection(&col).expect("export");
    let imported = import_postman_collection(&exported).expect("import");

    assert_eq!(imported.items.len(), auth_cases.len());
    for (i, expected_auth) in auth_cases.iter().enumerate() {
        let CollectionNode::Request(req) = &imported.items[i] else {
            panic!("expected request");
        };
        assert_eq!(&req.auth, expected_auth, "mismatch on auth index {i}");
    }
}

#[test]
fn postman_url_string_and_parts_without_raw() {
    let json = r#"{
        "info": { "name": "URL Forms", "schema": "https://schema.getpostman.com/json/collection/v2.1.0/collection.json" },
        "item": [
            {
                "name": "String URL",
                "request": "https://example.com/items?page=2&limit=10"
            },
            {
                "name": "Parts URL",
                "request": {
                    "method": "GET",
                    "url": {
                        "protocol": "https",
                        "host": ["api", "example", "com"],
                        "port": "8443",
                        "path": ["v1", "orders", ":id"],
                        "query": [{ "key": "expand", "value": "items" }]
                    }
                }
            }
        ]
    }"#;

    let col = import_postman_collection(json).expect("import");
    assert_eq!(col.items.len(), 2);

    let CollectionNode::Request(r1) = &col.items[0] else {
        panic!("expected r1");
    };
    assert_eq!(r1.url, "https://example.com/items");
    assert_eq!(r1.params.len(), 2);
    assert_eq!(r1.params[0].key, "page");
    assert_eq!(r1.params[0].value, "2");

    let CollectionNode::Request(r2) = &col.items[1] else {
        panic!("expected r2");
    };
    assert_eq!(r2.url, "https://api.example.com:8443/v1/orders/:id");
    assert_eq!(r2.params.len(), 1);
    assert_eq!(r2.params[0].key, "expand");
}

#[test]
fn postman_environment_round_trip() {
    let mut env = Environment::new("Staging");
    env.variables = vec![
        Variable::new("host", "https://staging.example.com"),
        Variable::secret("apiKey", "sk_staging_123"),
        Variable {
            key: "debug".into(),
            value: "true".into(),
            enabled: false,
            secret: false,
        },
    ];

    let json = export_postman_environment(&env).expect("export env");
    let imported = import_postman_environment(&json).expect("import env");

    assert_eq!(imported.id, env.id);
    assert_eq!(imported.name, "Staging");
    assert_eq!(imported.variables, env.variables);
}

#[test]
fn postman_real_file_compatibility_if_present() {
    let path = Path::new("/home/rafael.paula/Webhooks.postman_collection.json");
    if !path.exists() {
        return;
    }
    let content = std::fs::read_to_string(path).expect("read webhooks collection");
    let col = import_postman_collection(&content).expect("import webhooks collection");
    assert!(!col.name.is_empty());
    assert!(!col.items.is_empty());

    // Verify re-export and re-import succeed cleanly.
    let reexported = export_postman_collection(&col).expect("re-export webhooks collection");
    let reimported = import_postman_collection(&reexported).expect("re-import webhooks collection");
    assert_eq!(reimported.name, col.name);
    assert_eq!(reimported.items.len(), col.items.len());
}

// ============================================================================
// cURL Tests
// ============================================================================

#[test]
fn curl_import_multiline_flags_and_json() {
    let cmd = r#"curl -k -L -X POST \
      -H 'Content-Type: application/json' \
      -H "X-Custom: hello \"world\"" \
      -A 'ApiTester/1.0' \
      -b 'session=abc123' \
      -u 'admin:s3cr3t' \
      --data-raw '{"name":"Ada","active":true}' \
      'https://api.example.com/v1/users?role=admin'"#;

    let req = import_curl(cmd).expect("parse curl");
    assert_eq!(req.method, HttpMethod::Post);
    assert_eq!(req.url, "https://api.example.com/v1/users");
    assert_eq!(req.params.len(), 1);
    assert_eq!(req.params[0].key, "role");
    assert_eq!(req.params[0].value, "admin");
    assert_eq!(
        req.auth,
        AuthConfig::Basic {
            username: "admin".into(),
            password: "s3cr3t".into(),
        }
    );
    assert!(req
        .headers
        .iter()
        .any(|h| h.key == "X-Custom" && h.value == r#"hello "world""#));
    assert!(req
        .headers
        .iter()
        .any(|h| h.key == "User-Agent" && h.value == "ApiTester/1.0"));
    assert!(req
        .headers
        .iter()
        .any(|h| h.key == "Cookie" && h.value == "session=abc123"));
    assert_eq!(
        req.body,
        RequestBody::Raw {
            content: r#"{"name":"Ada","active":true}"#.into(),
            language: RawLanguage::Json,
        }
    );
}

#[test]
fn curl_import_form_urlencoded_and_get_flag() {
    let form_cmd = r#"curl -F 'name=Ada' -F 'avatar=@/tmp/pic.png;type=image/png' https://example.com/up"#;
    let form_req = import_curl(form_cmd).expect("form curl");
    assert_eq!(form_req.method, HttpMethod::Post);
    assert_eq!(
        form_req.body,
        RequestBody::FormData(vec![
            FormField::text("name", "Ada"),
            FormField {
                key: "avatar".into(),
                kind: FormFieldKind::File {
                    path: "/tmp/pic.png".into(),
                    content_type: Some("image/png".into()),
                },
                enabled: true,
                description: String::new(),
            },
        ])
    );

    let enc_cmd = r#"curl --data-urlencode 'a=1' --data-urlencode 'b=hello world' https://example.com/form"#;
    let enc_req = import_curl(enc_cmd).expect("urlencoded curl");
    assert_eq!(enc_req.method, HttpMethod::Post);
    assert_eq!(
        enc_req.body,
        RequestBody::UrlEncoded(vec![
            KeyValue::new("a", "1"),
            KeyValue::new("b", "hello world"),
        ])
    );

    let get_cmd = r#"curl -G -d 'q=rust' -d 'page=2' https://example.com/search"#;
    let get_req = import_curl(get_cmd).expect("get curl");
    assert_eq!(get_req.method, HttpMethod::Get);
    assert_eq!(get_req.body, RequestBody::None);
    assert_eq!(get_req.params.len(), 2);

    // Non-ASCII token starting with `-` must not panic on UTF-8 boundary slicing.
    let unicode_flag_req =
        import_curl("curl -é https://example.com/utf8").expect("non-ascii flag curl");
    assert_eq!(unicode_flag_req.url, "https://example.com/utf8");
}

#[test]
fn curl_export_with_variables_and_round_trip() {
    let mut vars = VariableContext::new();
    vars.set(VariableScope::Environment, "host", "https://api.example.com");
    vars.set(VariableScope::Environment, "tok", "secret_tok");

    let mut req = RequestItem::new("Update", HttpMethod::Patch, "{{host}}/items/1");
    req.params.push(KeyValue::new("force", "true"));
    req.headers
        .push(KeyValue::new("Content-Type", "application/json"));
    req.auth = AuthConfig::Bearer {
        token: "{{tok}}".into(),
    };
    req.body = RequestBody::Raw {
        content: r#"{"title":"O'Brien"}"#.into(),
        language: RawLanguage::Json,
    };

    let exported = export_curl(&req, &vars);
    assert!(exported.contains("https://api.example.com/items/1?force=true"));
    assert!(exported.contains("Authorization: Bearer secret_tok"));

    let reimported = import_curl(&exported).expect("reimport exported curl");
    assert_eq!(reimported.method, HttpMethod::Patch);
    assert_eq!(reimported.url, "https://api.example.com/items/1");
    assert_eq!(reimported.params.len(), 1);
    assert_eq!(
        reimported.body,
        RequestBody::Raw {
            content: r#"{"title":"O'Brien"}"#.into(),
            language: RawLanguage::Json,
        }
    );
}

// ============================================================================
// OpenAPI 3.x & Swagger 2.0 Tests
// ============================================================================

#[test]
fn openapi_v3_yaml_import_with_refs_and_tags() {
    let yaml = r#"
openapi: "3.0.3"
info:
  title: Petstore API
  description: Sample Petstore
servers:
  - url: https://{env}.petstore.example.com/v1
    variables:
      env:
        default: api
tags:
  - name: Pets
    description: Pet operations
paths:
  /pets/{petId}:
    parameters:
      - name: petId
        in: path
        required: true
        schema:
          type: string
    get:
      summary: Get a pet
      tags: [Pets]
      parameters:
        - $ref: '#/components/parameters/TraceHeader'
        - name: verbose
          in: query
          schema:
            type: boolean
            default: true
  /pets:
    post:
      summary: Create pet
      tags: [Pets]
      requestBody:
        content:
          application/json:
            schema:
              $ref: '#/components/schemas/NewPet'
  /health:
    get:
      operationId: healthCheck
components:
  parameters:
    TraceHeader:
      name: X-Trace-Id
      in: header
      example: trace-001
  schemas:
    NewPet:
      type: object
      properties:
        name:
          type: string
          example: Milo
        age:
          type: integer
          default: 3
"#;

    let col = import_openapi(yaml).expect("import openapi 3 yaml");
    assert_eq!(col.name, "Petstore API");
    assert_eq!(col.description, "Sample Petstore");
    assert!(col
        .variables
        .iter()
        .any(|v| v.key == "baseUrl" && v.value == "https://api.petstore.example.com/v1"));

    // 1 folder ("Pets") + 1 untagged request ("healthCheck")
    assert_eq!(col.items.len(), 2);

    let CollectionNode::Folder(pets_folder) = &col.items[0] else {
        panic!("expected Pets folder");
    };
    assert_eq!(pets_folder.name, "Pets");
    assert_eq!(pets_folder.description, "Pet operations");
    assert_eq!(pets_folder.items.len(), 2);

    let get_pet = pets_folder
        .items
        .iter()
        .find_map(|n| match n {
            CollectionNode::Request(r) if r.name == "Get a pet" => Some(r),
            _ => None,
        })
        .expect("Get a pet request");
    assert_eq!(
        get_pet.url,
        "https://api.petstore.example.com/v1/pets/{petId}"
    );
    assert!(get_pet
        .headers
        .iter()
        .any(|h| h.key == "X-Trace-Id" && h.value == "trace-001"));
    assert!(get_pet
        .params
        .iter()
        .any(|p| p.key == "verbose" && p.value == "true"));

    let create_pet = pets_folder
        .items
        .iter()
        .find_map(|n| match n {
            CollectionNode::Request(r) if r.name == "Create pet" => Some(r),
            _ => None,
        })
        .expect("Create pet request");
    assert_eq!(create_pet.method, HttpMethod::Post);
    match &create_pet.body {
        RequestBody::Raw { content, language } => {
            assert_eq!(*language, RawLanguage::Json);
            assert!(content.contains("Milo"));
            assert!(content.contains("3"));
        }
        other => panic!("expected raw JSON body, got {other:?}"),
    }

    let CollectionNode::Request(health) = &col.items[1] else {
        panic!("expected untagged healthCheck request");
    };
    assert_eq!(health.name, "healthCheck");
    assert_eq!(health.url, "https://api.petstore.example.com/v1/health");
}

#[test]
fn swagger_v2_json_import() {
    let json = r##"{
        "swagger": "2.0",
        "info": { "title": "Legacy API" },
        "host": "legacy.example.com",
        "basePath": "/api",
        "schemes": ["https"],
        "paths": {
            "/items": {
                "post": {
                    "summary": "Add item",
                    "tags": ["Inventory"],
                    "parameters": [
                        { "name": "dryRun", "in": "query", "type": "boolean", "default": false },
                        {
                            "name": "payload",
                            "in": "body",
                            "schema": { "$ref": "#/definitions/Item" }
                        }
                    ]
                }
            }
        },
        "definitions": {
            "Item": {
                "type": "object",
                "properties": {
                    "sku": { "type": "string", "example": "SKU-42" }
                }
            }
        }
    }"##;

    let col = import_openapi(json).expect("import swagger 2");
    assert_eq!(col.name, "Legacy API");
    assert_eq!(col.items.len(), 1);
    let CollectionNode::Folder(folder) = &col.items[0] else {
        panic!("expected Inventory folder");
    };
    assert_eq!(folder.name, "Inventory");
    let CollectionNode::Request(req) = &folder.items[0] else {
        panic!("expected request");
    };
    assert_eq!(req.url, "https://legacy.example.com/api/items");
    assert_eq!(req.method, HttpMethod::Post);
    assert!(req.params.iter().any(|p| p.key == "dryRun" && p.value == "false"));
    match &req.body {
        RequestBody::Raw { content, language } => {
            assert_eq!(*language, RawLanguage::Json);
            assert!(content.contains("SKU-42"));
        }
        other => panic!("expected raw json body, got {other:?}"),
    }
}

#[test]
fn openapi_rejects_unsupported_version() {
    let err = import_openapi(r#"{"openapi": "1.0.0", "info": {"title": "Old"}}"#).unwrap_err();
    assert!(matches!(err, ImportExportError::UnsupportedVersion(_)));
}

// ============================================================================
// HAR 1.2 Tests
// ============================================================================

#[test]
fn har_import_entries_bodies_and_examples() {
    let har = r#"{
        "log": {
            "version": "1.2",
            "creator": { "name": "Chrome DevTools", "version": "120.0" },
            "entries": [
                {
                    "request": {
                        "method": "GET",
                        "url": "https://api.example.com/search?q=rust",
                        "headers": [
                            { "name": ":method", "value": "GET" },
                            { "name": "Accept", "value": "application/json" }
                        ],
                        "queryString": [
                            { "name": "q", "value": "rust" }
                        ]
                    },
                    "response": {
                        "status": 200,
                        "statusText": "OK",
                        "headers": [{ "name": "Content-Type", "value": "application/json" }],
                        "content": { "text": "{\"hits\":5}" }
                    }
                },
                {
                    "request": {
                        "method": "POST",
                        "url": "https://api.example.com/login",
                        "headers": [],
                        "postData": {
                            "mimeType": "application/x-www-form-urlencoded",
                            "params": [
                                { "name": "user", "value": "ada" },
                                { "name": "pass", "value": "123" }
                            ]
                        }
                    }
                },
                {
                    "request": {
                        "method": "POST",
                        "url": "https://api.example.com/events",
                        "headers": [{ "name": "Content-Type", "value": "application/json" }],
                        "postData": {
                            "mimeType": "application/json",
                            "text": "{\"type\":\"click\"}"
                        }
                    }
                }
            ]
        }
    }"#;

    let col = import_har(har).expect("import har");
    assert_eq!(col.name, "HAR Import (Chrome DevTools)");
    assert_eq!(col.items.len(), 3);

    let CollectionNode::Request(r1) = &col.items[0] else {
        panic!("expected r1");
    };
    assert_eq!(r1.url, "https://api.example.com/search");
    assert_eq!(r1.params.len(), 1);
    assert_eq!(r1.headers.len(), 1); // `:method` pseudo-header stripped
    assert_eq!(r1.examples.len(), 1);
    assert_eq!(r1.examples[0].response.status, 200);
    assert_eq!(r1.examples[0].response.body_text(), r#"{"hits":5}"#);

    let CollectionNode::Request(r2) = &col.items[1] else {
        panic!("expected r2");
    };
    assert_eq!(
        r2.body,
        RequestBody::UrlEncoded(vec![
            KeyValue::new("user", "ada"),
            KeyValue::new("pass", "123"),
        ])
    );

    let CollectionNode::Request(r3) = &col.items[2] else {
        panic!("expected r3");
    };
    assert_eq!(
        r3.body,
        RequestBody::Raw {
            content: r#"{"type":"click"}"#.into(),
            language: RawLanguage::Json,
        }
    );
}

// ============================================================================
// Codegen Tests
// ============================================================================

#[test]
fn codegen_all_six_languages_resolve_variables_and_auth() {
    let mut vars = VariableContext::new();
    vars.set(VariableScope::Environment, "base", "https://api.example.com");
    vars.set(VariableScope::Environment, "token", "tok_xyz");

    let mut req = RequestItem::new("Create Item", HttpMethod::Post, "{{base}}/items");
    req.params.push(KeyValue::new("dry", "1"));
    req.headers
        .push(KeyValue::new("Content-Type", "application/json"));
    req.auth = AuthConfig::Bearer {
        token: "{{token}}".into(),
    };
    req.body = RequestBody::Raw {
        content: r#"{"name":"widget"}"#.into(),
        language: RawLanguage::Json,
    };

    let expected_url = "https://api.example.com/items?dry=1";

    let curl = generate_code(&req, CodegenLanguage::Curl, &vars);
    assert!(curl.starts_with("curl -X POST"));
    assert!(curl.contains(expected_url));
    assert!(curl.contains("Authorization: Bearer tok_xyz"));

    let rust = generate_code(&req, CodegenLanguage::RustReqwest, &vars);
    assert!(rust.contains("use reqwest::Client;"));
    assert!(rust.contains(expected_url));
    assert!(rust.contains("Bearer tok_xyz"));

    let py = generate_code(&req, CodegenLanguage::PythonRequests, &vars);
    assert!(py.contains("import requests"));
    assert!(py.contains(expected_url));
    assert!(py.contains("requests.request("));

    let js = generate_code(&req, CodegenLanguage::JavaScriptFetch, &vars);
    assert!(js.contains("await fetch("));
    assert!(js.contains(expected_url));
    assert!(js.contains("Bearer tok_xyz"));

    let go = generate_code(&req, CodegenLanguage::GoNetHttp, &vars);
    assert!(go.contains("http.NewRequest(\"POST\""));
    assert!(go.contains(expected_url));
    assert!(go.contains("req.Header.Set(\"Authorization\", \"Bearer tok_xyz\")"));

    let php = generate_code(&req, CodegenLanguage::PhpCurl, &vars);
    assert!(php.contains("curl_init()"));
    assert!(php.contains(expected_url));
    assert!(php.contains("CURLOPT_POSTFIELDS"));
}

#[test]
fn security_codegen_and_openapi_hardening() {
    let vars = VariableContext::new();

    // 1. Unsafe custom HTTP method must be shell-quoted in cURL export.
    let mut bad_method = RequestItem::new("Bad", HttpMethod::Custom("POST; id".into()), "https://example.com");
    bad_method.body = RequestBody::FormData(vec![FormField::text("note", "@secret.txt")]);
    let curl = export_curl(&bad_method, &vars);
    assert!(curl.starts_with("curl -X 'POST; id' 'https://example.com'"));
    assert!(curl.contains("--form-string 'note=@secret.txt'"));

    // 2. Binary path containing `*/` must not break out of JS block comment.
    let mut bin_req = RequestItem::new("Bin", HttpMethod::Post, "https://example.com");
    bin_req.body = RequestBody::Binary {
        path: "foo*/ alert(1) /*bar".into(),
    };
    let js = generate_code(&bin_req, CodegenLanguage::JavaScriptFetch, &vars);
    assert!(!js.contains("foo*/ alert(1)"));

    // 3. Cyclic $ref schema in OpenAPI must terminate cleanly without OOM/panic.
    let cyclic_openapi = r##"{
        "openapi": "3.0.0",
        "info": { "title": "Cyclic", "version": "1.0" },
        "paths": {
            "/node": {
                "post": {
                    "requestBody": {
                        "content": {
                            "application/json": {
                                "schema": { "$ref": "#/components/schemas/Node" }
                            }
                        }
                    }
                }
            }
        },
        "components": {
            "schemas": {
                "Node": {
                    "type": "object",
                    "properties": {
                        "a": { "$ref": "#/components/schemas/Node" },
                        "b": { "$ref": "#/components/schemas/Node" }
                    }
                }
            }
        }
    }"##;
    let col = import_openapi(cyclic_openapi).expect("cyclic openapi should not fail");
    assert_eq!(col.items.len(), 1);
}
