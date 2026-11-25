use apitester_core::models::auth::NtlmConfig;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use chrono::Utc;
use hmac::{Hmac, Mac};
use md4::{Digest as Md4Digest, Md4};
use md5::Md5;
use rand::RngCore;

use crate::{AuthApplication, AuthError};

type HmacMd5 = Hmac<Md5>;

const NTLM_SIGNATURE: &[u8; 8] = b"NTLMSSP\0";

// Negotiate flags (MS-NLMP 2.2.2.5)
const FLAG_NEGOTIATE_UNICODE: u32 = 0x0000_0001;
const FLAG_REQUEST_TARGET: u32 = 0x0000_0004;
const FLAG_NEGOTIATE_NTLM: u32 = 0x0000_0200;
const FLAG_NEGOTIATE_ALWAYS_SIGN: u32 = 0x0000_8000;
const FLAG_NEGOTIATE_EXTENDED_SESSIONSECURITY: u32 = 0x0008_0000;
const FLAG_NEGOTIATE_128: u32 = 0x2000_0000;
const FLAG_NEGOTIATE_OEM_DOMAIN_SUPPLIED: u32 = 0x0000_1000;
const FLAG_NEGOTIATE_OEM_WORKSTATION_SUPPLIED: u32 = 0x0000_2000;

/// Produces the initial `Authorization: NTLM <Type1>` header to begin an NTLM handshake.
pub fn apply_ntlm(cfg: &NtlmConfig) -> Result<AuthApplication, AuthError> {
    let type1 = build_type1_negotiate(cfg)?;
    Ok(AuthApplication::with_header(
        "Authorization",
        format!("NTLM {type1}"),
    ))
}

/// Completes an NTLM handshake by parsing a `WWW-Authenticate: NTLM <Type2>` challenge
/// and returning `Authorization: NTLM <Type3>`.
pub fn apply_ntlm_challenge(
    cfg: &NtlmConfig,
    www_authenticate: &str,
) -> Result<AuthApplication, AuthError> {
    let trimmed = www_authenticate.trim();
    let b64 = trimmed
        .strip_prefix("NTLM ")
        .or_else(|| trimmed.strip_prefix("ntlm "))
        .ok_or_else(|| {
            AuthError::InvalidChallenge("WWW-Authenticate header does not start with NTLM".into())
        })?
        .trim();

    let type3 = build_type3_authenticate(cfg, b64)?;
    Ok(AuthApplication::with_header(
        "Authorization",
        format!("NTLM {type3}"),
    ))
}

/// Builds a Base64-encoded NTLMSSP Type 1 (Negotiate) message.
pub fn build_type1_negotiate(cfg: &NtlmConfig) -> Result<String, AuthError> {
    let domain_bytes = cfg.domain.as_bytes();
    let workstation_bytes = cfg.workstation.as_bytes();

    let mut flags = FLAG_NEGOTIATE_UNICODE
        | FLAG_REQUEST_TARGET
        | FLAG_NEGOTIATE_NTLM
        | FLAG_NEGOTIATE_ALWAYS_SIGN
        | FLAG_NEGOTIATE_EXTENDED_SESSIONSECURITY
        | FLAG_NEGOTIATE_128;

    if !domain_bytes.is_empty() {
        flags |= FLAG_NEGOTIATE_OEM_DOMAIN_SUPPLIED;
    }
    if !workstation_bytes.is_empty() {
        flags |= FLAG_NEGOTIATE_OEM_WORKSTATION_SUPPLIED;
    }

    let domain_offset = 32u32;
    let domain_len_u32 = u32::try_from(domain_bytes.len())
        .map_err(|_| AuthError::InvalidConfig("NTLM domain too long".into()))?;
    let workstation_offset = domain_offset
        .checked_add(domain_len_u32)
        .ok_or_else(|| AuthError::InvalidConfig("NTLM Type 1 offset overflow".into()))?;

    let mut msg = Vec::with_capacity(32 + domain_bytes.len() + workstation_bytes.len());
    msg.extend_from_slice(NTLM_SIGNATURE);
    msg.extend_from_slice(&1u32.to_le_bytes()); // Type 1
    msg.extend_from_slice(&flags.to_le_bytes());
    write_sec_buf(&mut msg, domain_bytes.len(), domain_offset)?;
    write_sec_buf(&mut msg, workstation_bytes.len(), workstation_offset)?;
    msg.extend_from_slice(domain_bytes);
    msg.extend_from_slice(workstation_bytes);

    Ok(STANDARD.encode(msg))
}

/// Parses a Base64-encoded NTLMSSP Type 2 (Challenge) message and builds a Base64-encoded
/// NTLMSSP Type 3 (Authenticate) message using NTLMv2.
pub fn build_type3_authenticate(
    cfg: &NtlmConfig,
    type2_challenge_b64: &str,
) -> Result<String, AuthError> {
    let type2 = STANDARD
        .decode(type2_challenge_b64.trim())
        .map_err(|e| AuthError::InvalidChallenge(format!("invalid base64 NTLM Type 2: {e}")))?;

    if type2.len() < 32 || &type2[..8] != NTLM_SIGNATURE {
        return Err(AuthError::InvalidChallenge(
            "invalid NTLMSSP signature in Type 2 message".into(),
        ));
    }
    let msg_type = u32::from_le_bytes([type2[8], type2[9], type2[10], type2[11]]);
    if msg_type != 2 {
        return Err(AuthError::InvalidChallenge(format!(
            "expected NTLM Type 2 message, got Type {msg_type}"
        )));
    }

    let server_challenge: [u8; 8] = type2[24..32]
        .try_into()
        .map_err(|_| AuthError::InvalidChallenge("missing 8-byte server challenge".into()))?;

    // Extract optional TargetInfo buffer (bytes 40..48 if present).
    let target_info = if type2.len() >= 48 {
        let len = u16::from_le_bytes([type2[40], type2[41]]) as usize;
        let offset = u32::from_le_bytes([type2[44], type2[45], type2[46], type2[47]]) as usize;
        if len > 0 {
            offset
                .checked_add(len)
                .and_then(|end| type2.get(offset..end))
                .map(|slice| slice.to_vec())
                .unwrap_or_default()
        } else {
            Vec::new()
        }
    } else {
        Vec::new()
    };

    let mut client_challenge = [0u8; 8];
    rand::thread_rng().fill_bytes(&mut client_challenge);

    // NT Hash = MD4(UTF-16LE(password))
    let nt_hash = md4_utf16le(&cfg.password);

    // NTLMv2 Key = HMAC-MD5(NT_Hash, UTF-16LE(UPPER(username) + domain))
    let user_domain = format!("{}{}", cfg.username.to_uppercase(), cfg.domain);
    let ntlmv2_key = hmac_md5(&nt_hash, &to_utf16le(&user_domain))?;

    // Windows FILETIME: 100-nanosecond intervals since 1601-01-01.
    let unix_secs = Utc::now().timestamp() as u64;
    let filetime = (unix_secs + 11_644_473_600) * 10_000_000;

    // NTLMv2 client blob (Temp)
    let mut temp = Vec::with_capacity(32 + target_info.len());
    temp.extend_from_slice(&[0x01, 0x01, 0x00, 0x00]); // RespType, HiRespType, Reserved1
    temp.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]); // Reserved2
    temp.extend_from_slice(&filetime.to_le_bytes());
    temp.extend_from_slice(&client_challenge);
    temp.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]); // Reserved3
    temp.extend_from_slice(&target_info);
    temp.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]); // Terminator

    let mut nt_hmac_input = Vec::with_capacity(8 + temp.len());
    nt_hmac_input.extend_from_slice(&server_challenge);
    nt_hmac_input.extend_from_slice(&temp);
    let nt_proof = hmac_md5(&ntlmv2_key, &nt_hmac_input)?;

    let mut nt_response = Vec::with_capacity(nt_proof.len() + temp.len());
    nt_response.extend_from_slice(&nt_proof);
    nt_response.extend_from_slice(&temp);

    let mut lm_hmac_input = [0u8; 16];
    lm_hmac_input[..8].copy_from_slice(&server_challenge);
    lm_hmac_input[8..16].copy_from_slice(&client_challenge);
    let lm_proof = hmac_md5(&ntlmv2_key, &lm_hmac_input)?;

    let mut lm_response = Vec::with_capacity(24);
    lm_response.extend_from_slice(&lm_proof);
    lm_response.extend_from_slice(&client_challenge);

    let domain_utf16 = to_utf16le(&cfg.domain);
    let user_utf16 = to_utf16le(&cfg.username);
    let workstation_utf16 = to_utf16le(&cfg.workstation);

    let flags = FLAG_NEGOTIATE_UNICODE
        | FLAG_NEGOTIATE_NTLM
        | FLAG_NEGOTIATE_ALWAYS_SIGN
        | FLAG_NEGOTIATE_EXTENDED_SESSIONSECURITY
        | FLAG_NEGOTIATE_128;

    // Header size for Type 3 is 64 bytes (8 sig + 4 type + 6*8 secbufs + 4 flags).
    let mut offset = 64u32;
    let domain_off = offset;
    offset = offset
        .checked_add(u32::try_from(domain_utf16.len()).map_err(|_| AuthError::InvalidConfig("NTLM domain too long".into()))?)
        .ok_or_else(|| AuthError::InvalidConfig("NTLM Type 3 offset overflow".into()))?;
    let user_off = offset;
    offset = offset
        .checked_add(u32::try_from(user_utf16.len()).map_err(|_| AuthError::InvalidConfig("NTLM username too long".into()))?)
        .ok_or_else(|| AuthError::InvalidConfig("NTLM Type 3 offset overflow".into()))?;
    let ws_off = offset;
    offset = offset
        .checked_add(u32::try_from(workstation_utf16.len()).map_err(|_| AuthError::InvalidConfig("NTLM workstation too long".into()))?)
        .ok_or_else(|| AuthError::InvalidConfig("NTLM Type 3 offset overflow".into()))?;
    let lm_off = offset;
    offset = offset
        .checked_add(u32::try_from(lm_response.len()).map_err(|_| AuthError::InvalidChallenge("NTLM LM response too large".into()))?)
        .ok_or_else(|| AuthError::InvalidChallenge("NTLM Type 3 offset overflow".into()))?;
    let nt_off = offset;
    offset = offset
        .checked_add(u32::try_from(nt_response.len()).map_err(|_| AuthError::InvalidChallenge("NTLM NT response too large".into()))?)
        .ok_or_else(|| AuthError::InvalidChallenge("NTLM Type 3 offset overflow".into()))?;
    let session_key_off = offset;

    let mut msg = Vec::with_capacity(offset as usize);
    msg.extend_from_slice(NTLM_SIGNATURE);
    msg.extend_from_slice(&3u32.to_le_bytes()); // Type 3
    write_sec_buf(&mut msg, lm_response.len(), lm_off)?;
    write_sec_buf(&mut msg, nt_response.len(), nt_off)?;
    write_sec_buf(&mut msg, domain_utf16.len(), domain_off)?;
    write_sec_buf(&mut msg, user_utf16.len(), user_off)?;
    write_sec_buf(&mut msg, workstation_utf16.len(), ws_off)?;
    write_sec_buf(&mut msg, 0, session_key_off)?; // EncryptedRandomSessionKey
    msg.extend_from_slice(&flags.to_le_bytes());

    msg.extend_from_slice(&domain_utf16);
    msg.extend_from_slice(&user_utf16);
    msg.extend_from_slice(&workstation_utf16);
    msg.extend_from_slice(&lm_response);
    msg.extend_from_slice(&nt_response);

    Ok(STANDARD.encode(msg))
}

fn write_sec_buf(buf: &mut Vec<u8>, len: usize, offset: u32) -> Result<(), AuthError> {
    let len16 = u16::try_from(len).map_err(|_| {
        AuthError::InvalidChallenge(format!(
            "NTLM security buffer length {len} exceeds u16::MAX"
        ))
    })?;
    buf.extend_from_slice(&len16.to_le_bytes());
    buf.extend_from_slice(&len16.to_le_bytes());
    buf.extend_from_slice(&offset.to_le_bytes());
    Ok(())
}

fn to_utf16le(s: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len() * 2);
    for code_unit in s.encode_utf16() {
        out.extend_from_slice(&code_unit.to_le_bytes());
    }
    out
}

fn md4_utf16le(password: &str) -> [u8; 16] {
    let mut hasher = Md4::new();
    hasher.update(to_utf16le(password));
    hasher.finalize().into()
}

fn hmac_md5(key: &[u8], data: &[u8]) -> Result<[u8; 16], AuthError> {
    let mut mac =
        HmacMd5::new_from_slice(key).map_err(|e| AuthError::Crypto(e.to_string()))?;
    mac.update(data);
    let bytes = mac.finalize().into_bytes();
    let mut out = [0u8; 16];
    out.copy_from_slice(&bytes);
    Ok(out)
}
