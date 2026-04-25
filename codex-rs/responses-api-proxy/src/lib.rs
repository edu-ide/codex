use std::fs::File;
use std::fs::{self};
use std::io::Cursor;
use std::io::Read;
use std::io::Write;
use std::net::SocketAddr;
use std::net::TcpListener;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use anyhow::Result;
use anyhow::anyhow;
use clap::Parser;
use reqwest::Url;
use reqwest::blocking::Client;
use reqwest::header::AUTHORIZATION;
use reqwest::header::HOST;
use reqwest::header::HeaderMap;
use reqwest::header::HeaderName;
use reqwest::header::HeaderValue;
use serde::Serialize;
use tiny_http::Header;
use tiny_http::Method;
use tiny_http::Request;
use tiny_http::Response;
use tiny_http::Server;
use tiny_http::StatusCode;

mod autostart;
mod dump;
mod read_api_key;
mod sglang_qwen;
pub use autostart::SglangQwenProxy;
pub use autostart::maybe_start_sglang_qwen_proxy;
use dump::ExchangeDumper;
use read_api_key::read_auth_header_from_stdin;
use sglang_qwen::build_sglang_qwen_response_body;
use sglang_qwen::canonical_model_id_from_models_response;
use sglang_qwen::derive_chat_completions_url;
use sglang_qwen::derive_models_url;
use sglang_qwen::responses_to_chat_completions_request;

/// CLI arguments for the proxy.
#[derive(Debug, Clone, Parser)]
#[command(name = "responses-api-proxy", about = "Minimal OpenAI responses proxy")]
pub struct Args {
    /// Port to listen on. If not set, an ephemeral port is used.
    #[arg(long)]
    pub port: Option<u16>,

    /// Path to a JSON file to write startup info (single line). Includes {"port": <u16>}.
    #[arg(long, value_name = "FILE")]
    pub server_info: Option<PathBuf>,

    /// Enable HTTP shutdown endpoint at GET /shutdown
    #[arg(long)]
    pub http_shutdown: bool,

    /// Absolute URL the proxy should forward requests to (defaults to OpenAI).
    #[arg(long, default_value = "https://api.openai.com/v1/responses")]
    pub upstream_url: String,

    /// Directory where request/response dumps should be written as JSON.
    #[arg(long, value_name = "DIR")]
    pub dump_dir: Option<PathBuf>,

    /// Provider compatibility mode for the upstream.
    #[arg(long, value_enum, default_value_t = ProviderMode::Passthrough)]
    pub provider_mode: ProviderMode,
}

#[derive(Debug, Clone, Copy, clap::ValueEnum, PartialEq, Eq)]
pub enum ProviderMode {
    Passthrough,
    SglangQwen,
}

#[derive(Serialize)]
struct ServerInfo {
    port: u16,
    pid: u32,
}

struct ForwardConfig {
    upstream_url: Url,
    host_header: HeaderValue,
    provider_mode: ProviderMode,
}

/// Entry point for the library main, for parity with other crates.
pub fn run_main(args: Args) -> Result<()> {
    let auth_header = match args.provider_mode {
        ProviderMode::Passthrough => Some(read_auth_header_from_stdin()?),
        ProviderMode::SglangQwen => None,
    };

    let upstream_url = Url::parse(&args.upstream_url).context("parsing --upstream-url")?;
    let host = match (upstream_url.host_str(), upstream_url.port()) {
        (Some(host), Some(port)) => format!("{host}:{port}"),
        (Some(host), None) => host.to_string(),
        _ => return Err(anyhow!("upstream URL must include a host")),
    };
    let host_header =
        HeaderValue::from_str(&host).context("constructing Host header from upstream URL")?;

    let forward_config = Arc::new(ForwardConfig {
        upstream_url,
        host_header,
        provider_mode: args.provider_mode,
    });
    let dump_dir = args
        .dump_dir
        .map(ExchangeDumper::new)
        .transpose()
        .context("creating --dump-dir")?
        .map(Arc::new);

    let (listener, bound_addr) = bind_listener(args.port)?;
    if let Some(path) = args.server_info.as_ref() {
        write_server_info(path, bound_addr.port())?;
    }
    let server = Server::from_listener(listener, None)
        .map_err(|err| anyhow!("creating HTTP server: {err}"))?;
    let client = Arc::new(
        Client::builder()
            // Disable reqwest's 30s default so long-lived response streams keep flowing.
            .timeout(None::<Duration>)
            .build()
            .context("building reqwest client")?,
    );

    eprintln!("responses-api-proxy listening on {bound_addr}");

    let http_shutdown = args.http_shutdown;
    for request in server.incoming_requests() {
        let client = client.clone();
        let forward_config = forward_config.clone();
        let dump_dir = dump_dir.clone();
        std::thread::spawn(move || {
            if http_shutdown && request.method() == &Method::Get && request.url() == "/shutdown" {
                let _ = request.respond(Response::new_empty(StatusCode(200)));
                std::process::exit(0);
            }

            if let Err(e) = forward_request(
                &client,
                auth_header,
                &forward_config,
                dump_dir.as_deref(),
                request,
            ) {
                eprintln!("forwarding error: {e}");
            }
        });
    }

    Err(anyhow!("server stopped unexpectedly"))
}

fn bind_listener(port: Option<u16>) -> Result<(TcpListener, SocketAddr)> {
    let addr = SocketAddr::from(([127, 0, 0, 1], port.unwrap_or(0)));
    let listener = TcpListener::bind(addr).with_context(|| format!("failed to bind {addr}"))?;
    let bound = listener.local_addr().context("failed to read local_addr")?;
    Ok((listener, bound))
}

fn write_server_info(path: &Path, port: u16) -> Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)?;
    }

    let info = ServerInfo {
        port,
        pid: std::process::id(),
    };
    let mut data = serde_json::to_string(&info)?;
    data.push('\n');
    let mut f = File::create(path)?;
    f.write_all(data.as_bytes())?;
    Ok(())
}

fn forward_request(
    client: &Client,
    auth_header: Option<&'static str>,
    config: &ForwardConfig,
    dump_dir: Option<&ExchangeDumper>,
    req: Request,
) -> Result<()> {
    match config.provider_mode {
        ProviderMode::Passthrough => {
            forward_passthrough_request(client, auth_header, config, dump_dir, req)
        }
        ProviderMode::SglangQwen => forward_sglang_qwen_request(client, config, dump_dir, req),
    }
}

fn forward_passthrough_request(
    client: &Client,
    auth_header: Option<&'static str>,
    config: &ForwardConfig,
    dump_dir: Option<&ExchangeDumper>,
    mut req: Request,
) -> Result<()> {
    // Only allow POST /v1/responses exactly, no query string.
    let method = req.method().clone();
    let url_path = req.url().to_string();
    let allow = method == Method::Post && url_path == "/v1/responses";

    if !allow {
        let resp = Response::new_empty(StatusCode(403));
        let _ = req.respond(resp);
        return Ok(());
    }

    // Read request body
    let mut body = Vec::new();
    let reader = req.as_reader();
    reader.read_to_end(&mut body)?;

    let exchange_dump = dump_dir.and_then(|dump_dir| {
        dump_dir
            .dump_request(&method, &url_path, req.headers(), &body)
            .map_err(|err| {
                eprintln!("responses-api-proxy failed to dump request: {err}");
                err
            })
            .ok()
    });

    // Build headers for upstream, forwarding everything from the incoming
    // request except Authorization (we replace it below).
    let mut headers = HeaderMap::new();
    for header in req.headers() {
        let name_ascii = header.field.as_str();
        let lower = name_ascii.to_ascii_lowercase();
        if lower.as_str() == "authorization" || lower.as_str() == "host" {
            continue;
        }

        let header_name = match HeaderName::from_bytes(lower.as_bytes()) {
            Ok(name) => name,
            Err(_) => continue,
        };
        if let Ok(value) = HeaderValue::from_bytes(header.value.as_bytes()) {
            headers.append(header_name, value);
        }
    }

    // As part of our effort to to keep `auth_header` secret, we use a
    // combination of `from_static()` and `set_sensitive(true)`.
    if let Some(auth_header) = auth_header {
        let mut auth_header_value = HeaderValue::from_static(auth_header);
        auth_header_value.set_sensitive(true);
        headers.insert(AUTHORIZATION, auth_header_value);
    }

    headers.insert(HOST, config.host_header.clone());

    let upstream_resp = client
        .post(config.upstream_url.clone())
        .headers(headers)
        .body(body)
        .send()
        .context("forwarding request to upstream")?;

    // We have to create an adapter between a `reqwest::blocking::Response`
    // and a `tiny_http::Response`. Fortunately, `reqwest::blocking::Response`
    // implements `Read`, so we can use it directly as the body of the
    // `tiny_http::Response`.
    let status = upstream_resp.status();
    let mut response_headers = Vec::new();
    for (name, value) in upstream_resp.headers().iter() {
        // Skip headers that tiny_http manages itself.
        if matches!(
            name.as_str(),
            "content-length" | "transfer-encoding" | "connection" | "trailer" | "upgrade"
        ) {
            continue;
        }

        if let Ok(header) = Header::from_bytes(name.as_str().as_bytes(), value.as_bytes()) {
            response_headers.push(header);
        }
    }

    let content_length = upstream_resp.content_length().and_then(|len| {
        if len <= usize::MAX as u64 {
            Some(len as usize)
        } else {
            None
        }
    });

    let response_body: Box<dyn Read + Send> = if let Some(exchange_dump) = exchange_dump {
        let headers = upstream_resp.headers().clone();
        Box::new(exchange_dump.tee_response_body(status.as_u16(), &headers, upstream_resp))
    } else {
        Box::new(upstream_resp)
    };

    let response = Response::new(
        StatusCode(status.as_u16()),
        response_headers,
        response_body,
        content_length,
        None,
    );

    let _ = req.respond(response);
    Ok(())
}

fn forward_sglang_qwen_request(
    client: &Client,
    config: &ForwardConfig,
    dump_dir: Option<&ExchangeDumper>,
    mut req: Request,
) -> Result<()> {
    let method = req.method().clone();
    let url_path = req.url().to_string();
    let allow = (method == Method::Post && url_path == "/v1/responses")
        || (method == Method::Get && url_path == "/v1/models");

    if !allow {
        let resp = Response::new_empty(StatusCode(403));
        let _ = req.respond(resp);
        return Ok(());
    }

    let mut body = Vec::new();
    req.as_reader().read_to_end(&mut body)?;

    let exchange_dump = dump_dir.and_then(|dump_dir| {
        dump_dir
            .dump_request(&method, &url_path, req.headers(), &body)
            .map_err(|err| {
                eprintln!("responses-api-proxy failed to dump request: {err}");
                err
            })
            .ok()
    });

    if method == Method::Get && url_path == "/v1/models" {
        let models_url = derive_models_url(&config.upstream_url)?;
        return forward_simple_upstream_request(
            client,
            &models_url,
            &config.host_header,
            method,
            &body,
            exchange_dump,
            req,
        );
    }

    let request_json: serde_json::Value =
        serde_json::from_slice(&body).context("parsing responses request JSON")?;
    let wants_stream = request_json
        .get("stream")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let chat_body = responses_to_chat_completions_request(&request_json)?;
    let chat_url = derive_chat_completions_url(&config.upstream_url)?;
    let upstream_resp = client
        .post(chat_url)
        .header(HOST, config.host_header.clone())
        .header("content-type", "application/json")
        .body(serde_json::to_vec(&chat_body)?)
        .send()
        .context("forwarding transformed request to sglang chat/completions")?;

    let status = upstream_resp.status();
    let upstream_headers = upstream_resp.headers().clone();
    let upstream_body = upstream_resp
        .bytes()
        .context("reading transformed upstream response body")?;

    if !status.is_success() {
        return respond_with_bytes(
            req,
            status.as_u16(),
            upstream_headers,
            upstream_body.to_vec(),
            exchange_dump,
        );
    }

    let completion: serde_json::Value = serde_json::from_slice(&upstream_body)
        .context("parsing sglang chat/completions response")?;
    let response_model = fetch_sglang_server_model_id(client, config).unwrap_or_else(|_| {
        completion
            .get("model")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string()
    });
    let (content_type, response_body) = build_sglang_qwen_response_body(&completion, wants_stream)?;

    let mut response_headers = HeaderMap::new();
    response_headers.insert(
        reqwest::header::CONTENT_TYPE,
        HeaderValue::from_str(content_type).context("constructing content-type header")?,
    );
    if !response_model.is_empty() {
        response_headers.insert(
            HeaderName::from_static("openai-model"),
            HeaderValue::from_str(&response_model).context("constructing openai-model header")?,
        );
    }

    respond_with_bytes(req, 200, response_headers, response_body, exchange_dump)
}

fn fetch_sglang_server_model_id(client: &Client, config: &ForwardConfig) -> Result<String> {
    let models_url = derive_models_url(&config.upstream_url)?;
    let models_response = client
        .get(models_url)
        .header(HOST, config.host_header.clone())
        .send()
        .context("fetching sglang models metadata")?;
    let models_response = models_response
        .error_for_status()
        .context("sglang models metadata returned error status")?;
    let models_json: serde_json::Value = models_response
        .json()
        .context("parsing sglang models metadata JSON")?;
    canonical_model_id_from_models_response(&models_json)
        .ok_or_else(|| anyhow!("could not determine canonical SGLang model id"))
}

fn forward_simple_upstream_request(
    client: &Client,
    upstream_url: &Url,
    host_header: &HeaderValue,
    method: Method,
    body: &[u8],
    exchange_dump: Option<dump::ExchangeDump>,
    req: Request,
) -> Result<()> {
    let request_builder = match method {
        Method::Get => client.get(upstream_url.clone()),
        Method::Post => client.post(upstream_url.clone()).body(body.to_vec()),
        _ => {
            return Err(anyhow!(
                "unsupported method for upstream forwarding: {method:?}"
            ));
        }
    }
    .header(HOST, host_header.clone());

    let upstream_resp = request_builder
        .send()
        .context("forwarding request to upstream")?;
    let status = upstream_resp.status();
    let headers = upstream_resp.headers().clone();
    let body = upstream_resp
        .bytes()
        .context("reading forwarded upstream response body")?;
    respond_with_bytes(req, status.as_u16(), headers, body.to_vec(), exchange_dump)
}

fn respond_with_bytes(
    req: Request,
    status: u16,
    headers: HeaderMap,
    body: Vec<u8>,
    exchange_dump: Option<dump::ExchangeDump>,
) -> Result<()> {
    let mut response_headers = Vec::new();
    for (name, value) in headers.iter() {
        if matches!(
            name.as_str(),
            "content-length" | "transfer-encoding" | "connection" | "trailer" | "upgrade"
        ) {
            continue;
        }

        if let Ok(header) = Header::from_bytes(name.as_str().as_bytes(), value.as_bytes()) {
            response_headers.push(header);
        }
    }

    let content_length = body.len();
    let response_body: Box<dyn Read + Send> = if let Some(exchange_dump) = exchange_dump {
        Box::new(exchange_dump.tee_response_body(status, &headers, Cursor::new(body)))
    } else {
        Box::new(Cursor::new(body))
    };

    let response = Response::new(
        StatusCode(status),
        response_headers,
        response_body,
        Some(content_length),
        None,
    );
    let _ = req.respond(response);
    Ok(())
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use serde_json::json;

    #[test]
    fn transforms_qwen_tool_markup_into_function_call_item() {
        let completion = json!({
            "id": "chatcmpl-1",
            "created": 1776400000,
            "model": "/home/sk/models/Qwen3.6-35B-A3B",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "<tool_call>\n<function=get_weather>\n<parameter=city>\n서울\n</parameter>\n</function>\n</tool_call>",
                    "tool_calls": []
                },
                "finish_reason": "tool_calls"
            }],
            "usage": {
                "prompt_tokens": 10,
                "completion_tokens": 5,
                "total_tokens": 15
            }
        });

        let response = super::sglang_qwen::transform_chat_completion_to_responses_json(&completion)
            .expect("transform completion");

        assert_eq!(
            response["output"],
            json!([{
                "type": "function_call",
                "call_id": "call_1",
                "name": "get_weather",
                "arguments": "{\"city\":\"서울\"}"
            }])
        );
    }

    #[test]
    fn converts_responses_request_to_chat_completions_request() {
        let request = json!({
            "model": "ilhae",
            "instructions": "You are helpful.",
            "input": [{
                "type": "message",
                "role": "user",
                "content": [{"type": "input_text", "text": "서울 날씨를 조회해."}]
            }],
            "tools": [{
                "type": "function",
                "name": "get_weather",
                "description": "날씨 조회",
                "parameters": {
                    "type": "object",
                    "properties": {"city": {"type": "string"}},
                    "required": ["city"]
                }
            }],
            "tool_choice": "auto",
            "parallel_tool_calls": true,
            "stream": true
        });

        let chat = super::sglang_qwen::responses_to_chat_completions_request(&request)
            .expect("chat request");

        assert_eq!(
            chat,
            json!({
                "model": "ilhae",
                "messages": [
                    {"role": "system", "content": "You are helpful."},
                    {"role": "user", "content": "서울 날씨를 조회해."}
                ],
                "tools": [{
                    "type": "function",
                    "function": {
                        "name": "get_weather",
                        "description": "날씨 조회",
                        "parameters": {
                            "type": "object",
                            "properties": {"city": {"type": "string"}},
                            "required": ["city"]
                        }
                    }
                }],
                "tool_choice": "auto",
                "parallel_tool_calls": true,
                "chat_template_kwargs": {
                    "enable_thinking": false
                },
                "stream": false
            })
        );
    }

    #[test]
    fn converts_developer_messages_into_system_messages() {
        let request = json!({
            "model": "ilhae",
            "input": [{
                "type": "message",
                "role": "developer",
                "content": [{"type": "input_text", "text": "Always answer tersely."}]
            }]
        });

        let chat = super::sglang_qwen::responses_to_chat_completions_request(&request)
            .expect("chat request");

        assert_eq!(
            chat["messages"],
            json!([{
                "role": "system",
                "content": "Always answer tersely."
            }])
        );
    }

    #[test]
    fn merges_instructions_and_developer_messages_into_single_system_message() {
        let request = json!({
            "model": "ilhae",
            "instructions": "Primary system rule.",
            "input": [
                {
                    "type": "message",
                    "role": "developer",
                    "content": [{"type": "input_text", "text": "Secondary developer rule."}]
                },
                {
                    "type": "message",
                    "role": "user",
                    "content": [{"type": "input_text", "text": "안녕"}]
                }
            ]
        });

        let chat = super::sglang_qwen::responses_to_chat_completions_request(&request)
            .expect("chat request");

        assert_eq!(
            chat["messages"],
            json!([
                {
                    "role": "system",
                    "content": "Primary system rule.\n\nSecondary developer rule."
                },
                {
                    "role": "user",
                    "content": "안녕"
                }
            ])
        );
    }

    #[test]
    fn uses_canonical_model_id_from_single_models_entry() {
        let models = json!({
            "object": "list",
            "data": [{
                "id": "/home/sk/models/Qwen3.6-35B-A3B",
                "object": "model"
            }]
        });

        assert_eq!(
            super::sglang_qwen::canonical_model_id_from_models_response(&models).as_deref(),
            Some("/home/sk/models/Qwen3.6-35B-A3B")
        );
    }

    #[test]
    fn strips_orphan_think_closing_tag_prefix() {
        let completion = json!({
            "id": "resp_123",
            "created": 1,
            "model": "ilhae",
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "Internal reasoning line 1.\nInternal reasoning line 2.\n</think>\n\n안녕하세요!"
                }
            }]
        });

        let response = super::sglang_qwen::transform_chat_completion_to_responses_json(&completion)
            .expect("response JSON");

        assert_eq!(
            response["output"][0]["content"][0]["text"],
            json!("안녕하세요!")
        );
    }
}
