use apitester_core::models::{
    ApiKeyLocation, AsapConfig, AuthConfig, AwsV4Config, DigestConfig, EdgeGridConfig, HawkConfig,
    JwtConfig, NtlmConfig, OAuth1Config, OAuth2Config, OAuth2GrantType,
};
use egui::{ComboBox, Grid, Ui};

const AUTH_LABELS: &[&str] = &[
    "Inherit auth from parent",
    "No Auth",
    "Bearer Token",
    "Basic Auth",
    "API Key",
    "OAuth 2.0",
    "AWS Signature (SigV4)",
    "JWT Bearer",
    "Digest Auth",
    "OAuth 1.0",
    "NTLM Authentication",
    "Hawk Authentication",
    "Akamai EdgeGrid",
    "ASAP (Atlassian)",
];

fn auth_index(auth: &AuthConfig) -> usize {
    match auth {
        AuthConfig::Inherit => 0,
        AuthConfig::NoAuth => 1,
        AuthConfig::Bearer { .. } => 2,
        AuthConfig::Basic { .. } => 3,
        AuthConfig::ApiKey { .. } => 4,
        AuthConfig::OAuth2(_) => 5,
        AuthConfig::AwsV4(_) => 6,
        AuthConfig::Jwt(_) => 7,
        AuthConfig::Digest(_) => 8,
        AuthConfig::OAuth1(_) => 9,
        AuthConfig::Ntlm(_) => 10,
        AuthConfig::Hawk(_) => 11,
        AuthConfig::EdgeGrid(_) => 12,
        AuthConfig::Asap(_) => 13,
    }
}

fn auth_from_index(idx: usize) -> AuthConfig {
    match idx {
        0 => AuthConfig::Inherit,
        1 => AuthConfig::NoAuth,
        2 => AuthConfig::Bearer {
            token: String::new(),
        },
        3 => AuthConfig::Basic {
            username: String::new(),
            password: String::new(),
        },
        4 => AuthConfig::ApiKey {
            key: "X-API-Key".into(),
            value: String::new(),
            add_to: ApiKeyLocation::Header,
        },
        5 => AuthConfig::OAuth2(OAuth2Config::default()),
        6 => AuthConfig::AwsV4(AwsV4Config::default()),
        7 => AuthConfig::Jwt(JwtConfig::default()),
        8 => AuthConfig::Digest(DigestConfig::default()),
        9 => AuthConfig::OAuth1(OAuth1Config::default()),
        10 => AuthConfig::Ntlm(NtlmConfig::default()),
        11 => AuthConfig::Hawk(HawkConfig::default()),
        12 => AuthConfig::EdgeGrid(EdgeGridConfig::default()),
        13 => AuthConfig::Asap(AsapConfig::default()),
        _ => AuthConfig::NoAuth,
    }
}

/// Renders a dynamic authentication editor supporting all 13 Postman auth types + Inherit.
pub fn auth_editor_ui(ui: &mut Ui, id_salt: &str, auth: &mut AuthConfig) -> bool {
    let mut changed = false;
    let current = auth_index(auth);
    let mut selected = current;

    ui.horizontal(|ui| {
        ui.label("Auth Type:");
        ComboBox::from_id_salt(format!("{id_salt}_type"))
            .width(220.0)
            .selected_text(AUTH_LABELS[selected])
            .show_ui(ui, |ui| {
                for (idx, label) in AUTH_LABELS.iter().enumerate() {
                    ui.selectable_value(&mut selected, idx, *label);
                }
            });
    });

    if selected != current {
        *auth = auth_from_index(selected);
        changed = true;
    }

    ui.add_space(6.0);

    match auth {
        AuthConfig::Inherit => {
            ui.weak("Inherits authentication settings from the parent folder or collection.");
        }
        AuthConfig::NoAuth => {
            ui.weak("This request does not use any authorization.");
        }
        AuthConfig::Bearer { token } => {
            Grid::new(format!("{id_salt}_bearer")).num_columns(2).show(ui, |ui| {
                changed |= row_text(ui, "Token", token, false);
            });
        }
        AuthConfig::Basic { username, password } => {
            Grid::new(format!("{id_salt}_basic")).num_columns(2).show(ui, |ui| {
                changed |= row_text(ui, "Username", username, false);
                changed |= row_text(ui, "Password", password, true);
            });
        }
        AuthConfig::ApiKey { key, value, add_to } => {
            Grid::new(format!("{id_salt}_apikey")).num_columns(2).show(ui, |ui| {
                changed |= row_text(ui, "Key", key, false);
                changed |= row_text(ui, "Value", value, false);
                changed |= row_location(ui, &format!("{id_salt}_ak_loc"), "Add to", add_to);
            });
        }
        AuthConfig::OAuth2(cfg) => {
            Grid::new(format!("{id_salt}_oauth2")).num_columns(2).show(ui, |ui| {
                ui.label("Grant Type");
                ComboBox::from_id_salt(format!("{id_salt}_o2_grant"))
                    .selected_text(format!("{:?}", cfg.grant_type))
                    .show_ui(ui, |ui| {
                        changed |= ui
                            .selectable_value(
                                &mut cfg.grant_type,
                                OAuth2GrantType::AuthorizationCode,
                                "Authorization Code",
                            )
                            .changed();
                        changed |= ui
                            .selectable_value(
                                &mut cfg.grant_type,
                                OAuth2GrantType::AuthorizationCodePkce,
                                "Authorization Code (PKCE)",
                            )
                            .changed();
                        changed |= ui
                            .selectable_value(
                                &mut cfg.grant_type,
                                OAuth2GrantType::ClientCredentials,
                                "Client Credentials",
                            )
                            .changed();
                        changed |= ui
                            .selectable_value(&mut cfg.grant_type, OAuth2GrantType::Password, "Password")
                            .changed();
                        changed |= ui
                            .selectable_value(&mut cfg.grant_type, OAuth2GrantType::Implicit, "Implicit")
                            .changed();
                    });
                ui.end_row();
                changed |= row_text(ui, "Access Token", &mut cfg.access_token, false);
                changed |= row_text(ui, "Header Prefix", &mut cfg.header_prefix, false);
                changed |= row_text(ui, "Auth URL", &mut cfg.auth_url, false);
                changed |= row_text(ui, "Token URL", &mut cfg.token_url, false);
                changed |= row_text(ui, "Client ID", &mut cfg.client_id, false);
                changed |= row_text(ui, "Client Secret", &mut cfg.client_secret, true);
                changed |= row_text(ui, "Scope", &mut cfg.scope, false);
                changed |= row_location(ui, &format!("{id_salt}_o2_loc"), "Add token to", &mut cfg.add_token_to);
            });
        }
        AuthConfig::AwsV4(cfg) => {
            Grid::new(format!("{id_salt}_awsv4")).num_columns(2).show(ui, |ui| {
                changed |= row_text(ui, "Access Key", &mut cfg.access_key, false);
                changed |= row_text(ui, "Secret Key", &mut cfg.secret_key, true);
                changed |= row_text(ui, "AWS Region", &mut cfg.region, false);
                changed |= row_text(ui, "Service Name", &mut cfg.service, false);
                let mut token = cfg.session_token.clone().unwrap_or_default();
                if row_text(ui, "Session Token", &mut token, false) {
                    cfg.session_token = if token.is_empty() { None } else { Some(token) };
                    changed = true;
                }
                changed |= row_location(ui, &format!("{id_salt}_aws_loc"), "Add to", &mut cfg.add_to);
            });
        }
        AuthConfig::Jwt(cfg) => {
            Grid::new(format!("{id_salt}_jwt")).num_columns(2).show(ui, |ui| {
                changed |= row_text(ui, "Algorithm", &mut cfg.algorithm, false);
                changed |= row_text(ui, "Secret", &mut cfg.secret, true);
                ui.label("Secret Base64");
                changed |= ui.checkbox(&mut cfg.is_secret_base64, "").changed();
                ui.end_row();
                changed |= row_text(ui, "Payload (JSON)", &mut cfg.payload, false);
                changed |= row_text(ui, "Header Prefix", &mut cfg.header_prefix, false);
                changed |= row_location(ui, &format!("{id_salt}_jwt_loc"), "Add to", &mut cfg.add_to);
            });
        }
        AuthConfig::Digest(cfg) => {
            Grid::new(format!("{id_salt}_digest")).num_columns(2).show(ui, |ui| {
                changed |= row_text(ui, "Username", &mut cfg.username, false);
                changed |= row_text(ui, "Password", &mut cfg.password, true);
                changed |= row_text(ui, "Realm", &mut cfg.realm, false);
                changed |= row_text(ui, "Nonce", &mut cfg.nonce, false);
                changed |= row_text(ui, "Algorithm", &mut cfg.algorithm, false);
                changed |= row_text(ui, "qop", &mut cfg.qop, false);
            });
        }
        AuthConfig::OAuth1(cfg) => {
            Grid::new(format!("{id_salt}_oauth1")).num_columns(2).show(ui, |ui| {
                changed |= row_text(ui, "Consumer Key", &mut cfg.consumer_key, false);
                changed |= row_text(ui, "Consumer Secret", &mut cfg.consumer_secret, true);
                changed |= row_text(ui, "Access Token", &mut cfg.token, false);
                changed |= row_text(ui, "Token Secret", &mut cfg.token_secret, true);
                changed |= row_text(ui, "Signature Method", &mut cfg.signature_method, false);
            });
        }
        AuthConfig::Ntlm(cfg) => {
            Grid::new(format!("{id_salt}_ntlm")).num_columns(2).show(ui, |ui| {
                changed |= row_text(ui, "Username", &mut cfg.username, false);
                changed |= row_text(ui, "Password", &mut cfg.password, true);
                changed |= row_text(ui, "Domain", &mut cfg.domain, false);
                changed |= row_text(ui, "Workstation", &mut cfg.workstation, false);
            });
        }
        AuthConfig::Hawk(cfg) => {
            Grid::new(format!("{id_salt}_hawk")).num_columns(2).show(ui, |ui| {
                changed |= row_text(ui, "Hawk Auth ID", &mut cfg.auth_id, false);
                changed |= row_text(ui, "Hawk Auth Key", &mut cfg.auth_key, true);
                changed |= row_text(ui, "Algorithm", &mut cfg.algorithm, false);
                changed |= row_text(ui, "User", &mut cfg.user, false);
            });
        }
        AuthConfig::EdgeGrid(cfg) => {
            Grid::new(format!("{id_salt}_edgegrid")).num_columns(2).show(ui, |ui| {
                changed |= row_text(ui, "Access Token", &mut cfg.access_token, false);
                changed |= row_text(ui, "Client Token", &mut cfg.client_token, false);
                changed |= row_text(ui, "Client Secret", &mut cfg.client_secret, true);
                changed |= row_text(ui, "Base URL", &mut cfg.base_url, false);
            });
        }
        AuthConfig::Asap(cfg) => {
            Grid::new(format!("{id_salt}_asap")).num_columns(2).show(ui, |ui| {
                changed |= row_text(ui, "Algorithm", &mut cfg.alg, false);
                changed |= row_text(ui, "Issuer", &mut cfg.issuer, false);
                changed |= row_text(ui, "Key ID (kid)", &mut cfg.kid, false);
                changed |= row_text(ui, "Audience", &mut cfg.audience, false);
                changed |= row_text(ui, "Private Key (PEM)", &mut cfg.private_key, true);
            });
        }
    }

    changed
}

fn row_text(ui: &mut Ui, label: &str, value: &mut String, secret: bool) -> bool {
    ui.label(label);
    let resp = ui.add(
        egui::TextEdit::singleline(value)
            .password(secret)
            .desired_width(340.0),
    );
    ui.end_row();
    resp.changed()
}

fn row_location(ui: &mut Ui, id: &str, label: &str, loc: &mut ApiKeyLocation) -> bool {
    let mut changed = false;
    ui.label(label);
    ComboBox::from_id_salt(id)
        .selected_text(match loc {
            ApiKeyLocation::Header => "Header",
            ApiKeyLocation::Query => "Query Params",
        })
        .show_ui(ui, |ui| {
            changed |= ui
                .selectable_value(loc, ApiKeyLocation::Header, "Header")
                .changed();
            changed |= ui
                .selectable_value(loc, ApiKeyLocation::Query, "Query Params")
                .changed();
        });
    ui.end_row();
    changed
}
