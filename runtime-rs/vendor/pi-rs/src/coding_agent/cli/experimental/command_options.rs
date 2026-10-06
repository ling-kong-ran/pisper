//! Port of upstream `coding-agent/src/cli/experimental/command-options.ts`
//! (sha256 d6251c5e21bd…): shared option definitions for the experimental
//! server/client commands, including the `--connect` transport-address and
//! `--auth-token(-file)` grammar.
//!
//! Upstream parses with the WHATWG `URL` parser; the port implements the
//! equivalent grammar for the non-special schemes the CLI accepts
//! (`radius:`, `unix:`, anything else → "Unsupported transport"), including
//! the href-normalization check (protocol/hostname lower-cased).

use crate::protocol::protocol::is_server_id;

use super::command::{value_option, CommandOption, OptionValue, ParsedCommandInput};

/// Upstream `AuthInput`.
#[derive(Debug, Clone, PartialEq)]
pub enum AuthInput {
    Token { token: String },
    File { path: String },
}

/// Upstream `TransportAddress`.
#[derive(Debug, Clone, PartialEq)]
pub enum TransportAddress {
    Unix { path: String },
    Radius { server_id: String },
}

/// Upstream `authTokenOption`.
pub fn auth_token_option() -> CommandOption {
    super::command::string_option("--auth-token", false)
}

/// Upstream `authTokenFileOption`.
pub fn auth_token_file_option() -> CommandOption {
    super::command::string_option("--auth-token-file", false)
}

/// WHATWG-URL-shaped parts the transport grammar consults.
struct UrlParts {
    protocol: String,
    username: String,
    password: String,
    hostname: String,
    port: String,
    pathname: String,
    search: String,
    hash: String,
    href: String,
}

fn parse_url(value: &str) -> Result<UrlParts, ()> {
    let colon = value.find(':').ok_or(())?;
    let (raw_protocol, _rest) = value.split_at(colon + 1);
    let protocol = raw_protocol.to_ascii_lowercase();
    let protocol_len = raw_protocol.len();
    if !value
        .as_bytes()
        .get(protocol_len..)
        .is_some_and(|rest| rest.starts_with(b"//"))
    {
        // Opaque path (no authority): only the protocol is relevant for the
        // CLI grammar; node would form `scheme:<path>`.
        let tail = &value[protocol_len..];
        let (before_hash, hash) = match tail.find('#') {
            Some(hash) => (&tail[..hash], tail[hash..].to_string()),
            None => (tail, String::new()),
        };
        let href = format!("{protocol}{before_hash}{hash}");
        return Ok(UrlParts {
            protocol,
            username: String::new(),
            password: String::new(),
            hostname: String::new(),
            port: String::new(),
            pathname: before_hash.to_string(),
            search: String::new(),
            hash,
            href,
        });
    }

    let after = &value[protocol_len + 2..];
    let authority_end = after.find(['/', '?', '#']).unwrap_or(after.len());
    let authority = &after[..authority_end];
    let tail = &after[authority_end..];

    let (userinfo, hostport) = match authority.rfind('@') {
        Some(at) => (&authority[..at], &authority[at + 1..]),
        None => ("", authority),
    };
    let (username, password) = match userinfo.find(':') {
        Some(colon) => (&userinfo[..colon], &userinfo[colon + 1..]),
        None => (userinfo, ""),
    };
    let (hostname, port) = match hostport.rfind(':') {
        Some(colon) if !hostport[colon + 1..].is_empty() && !hostport.starts_with('[') => {
            let port = &hostport[colon + 1..];
            if port.bytes().any(|byte| !byte.is_ascii_digit()) {
                return Err(());
            }
            (&hostport[..colon], port.to_string())
        }
        _ => (hostport, String::new()),
    };

    let (before_hash, hash) = match tail.find('#') {
        Some(hash) => (&tail[..hash], tail[hash..].to_string()),
        None => (tail, String::new()),
    };
    let (pathname, search) = match before_hash.find('?') {
        Some(question) => (
            &before_hash[..question],
            before_hash[question..].to_string(),
        ),
        None => (before_hash, String::new()),
    };

    let mut href = format!("{protocol}//");
    if !userinfo.is_empty() {
        href.push_str(userinfo);
        href.push('@');
    }
    href.push_str(&hostname.to_ascii_lowercase());
    if !port.is_empty() {
        href.push(':');
        href.push_str(&port);
    }
    href.push_str(pathname);
    href.push_str(&search);
    href.push_str(&hash);

    Ok(UrlParts {
        protocol,
        username: username.to_string(),
        password: password.to_string(),
        hostname: hostname.to_ascii_lowercase(),
        port,
        pathname: pathname.to_string(),
        search,
        hash,
        href,
    })
}

/// Upstream `parseTransportAddress`.
pub fn parse_transport_address(value: &str) -> Result<TransportAddress, String> {
    let url = parse_url(value).map_err(|_| format!("Invalid --connect address \"{value}\""))?;
    if url.protocol == "radius:" {
        if !url.username.is_empty()
            || !url.password.is_empty()
            || !url.port.is_empty()
            || (!url.pathname.is_empty() && url.pathname != "/")
            || !url.search.is_empty()
            || !url.hash.is_empty()
            || value != format!("radius://{}{}", url.hostname, url.pathname)
        {
            return Err(format!("Invalid --connect address \"{value}\""));
        }
        let server_id = url.hostname.clone();
        if !is_server_id(&server_id) {
            return Err(
                "Radius transport address requires a lowercase UUIDv4 server ID".to_string(),
            );
        }
        return Ok(TransportAddress::Radius { server_id });
    }
    if url.protocol != "unix:" {
        return Err(format!(
            "Unsupported --connect transport \"{}\"",
            url.protocol
        ));
    }
    if !url.hostname.is_empty()
        || !url.port.is_empty()
        || !url.username.is_empty()
        || !url.password.is_empty()
    {
        return Err("Unix transport address must not include an authority".to_string());
    }
    if !value.starts_with("unix:///")
        || value.starts_with("unix:////")
        || value.contains('?')
        || value.contains('#')
        || url.href != value
    {
        return Err(format!("Invalid --connect address \"{value}\""));
    }
    let Ok(path) = percent_decode(&url.pathname) else {
        return Err(format!("Invalid --connect address \"{value}\""));
    };
    if path.contains('\0') {
        return Err(format!("Invalid --connect address \"{value}\""));
    }
    if !path.starts_with('/') {
        return Err("Unix transport address requires an absolute path".to_string());
    }
    Ok(TransportAddress::Unix { path })
}

/// `decodeURIComponent` (invalid `%` escapes reject, like upstream's try/catch).
fn percent_decode(value: &str) -> Result<String, ()> {
    let bytes = value.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let hex = bytes.get(index + 1..index + 3).ok_or(())?;
            let high = (hex[0] as char).to_digit(16).ok_or(())?;
            let low = (hex[1] as char).to_digit(16).ok_or(())?;
            out.push((high * 16 + low) as u8);
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(out).map_err(|_| ())
}

/// Upstream `connectOption`.
pub fn connect_option() -> CommandOption {
    value_option(
        "--connect",
        |value| match parse_transport_address(value) {
            Ok(address) => Ok(OptionValue::Connect(address)),
            Err(error) => Err(error),
        },
        false,
    )
}

/// Upstream `parseAuth` / `parseAuthInput`.
pub fn parse_auth(input: &ParsedCommandInput) -> (Option<AuthInput>, Vec<String>) {
    let auth_token = input.value("--auth-token");
    let auth_token_file = input.value("--auth-token-file");
    let auth_token = auth_token.and_then(OptionValue::as_text);
    let auth_token_file = auth_token_file.and_then(OptionValue::as_text);
    match (auth_token, auth_token_file) {
        (Some(_), Some(_)) => (
            None,
            vec!["--auth-token and --auth-token-file are mutually exclusive".to_string()],
        ),
        (Some(token), None) => (
            Some(AuthInput::Token {
                token: token.to_string(),
            }),
            Vec::new(),
        ),
        (None, Some(path)) => (
            Some(AuthInput::File {
                path: path.to_string(),
            }),
            Vec::new(),
        ),
        (None, None) => (None, Vec::new()),
    }
}

/// Upstream `unsupportedOptions`.
pub fn unsupported_options(command: &str, input: &ParsedCommandInput) -> Vec<String> {
    if input.remaining_args.is_empty() {
        return Vec::new();
    }
    vec![format!(
        "The experimental {command} command does not support existing CLI options yet"
    )]
}
