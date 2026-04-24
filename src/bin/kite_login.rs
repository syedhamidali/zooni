//! Kite Connect daily login helper.
//!
//! Usage:
//!   zooni-kite-login config.toml
//!   zooni-kite-login config.toml --port 8383
//!
//! Flow:
//!   1. Reads `[zerodha]` credentials from config.
//!   2. Prints the Kite login URL — open it in a browser.
//!   3. Starts a tiny HTTP server on localhost:8383 (configurable).
//!   4. After user logs in, Kite redirects to `http://localhost:8383/?request_token=...`
//!   5. Exchanges the request_token for an access_token via POST /session/token.
//!   6. Prints the access_token and optionally writes it to a file.
//!
//! The access_token is valid until ~06:00 IST the next day.

use anyhow::{Context, Result, anyhow};
use std::io::Write;
use std::process::Command;

fn open_browser(url: &str) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    Command::new("open").arg(url).spawn()?;
    #[cfg(target_os = "linux")]
    Command::new("xdg-open").arg(url).spawn()?;
    #[cfg(target_os = "windows")]
    Command::new("cmd").args(["/C", "start", url]).spawn()?;
    Ok(())
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();

    let config_path = args
        .iter()
        .find(|a| !a.starts_with('-') && *a != &args[0])
        .map(|s| s.as_str())
        .unwrap_or("config.toml");

    let mut port: u16 = 8383;
    for (i, arg) in args.iter().enumerate() {
        if arg == "--port" {
            if let Some(p) = args.get(i + 1) {
                port = p.parse().context("invalid --port value")?;
            }
        }
    }

    let cfg = zooni::config::BotConfig::load(config_path)
        .with_context(|| format!("loading config from {config_path}"))?;

    let zerodha_cfg = cfg
        .zerodha
        .ok_or_else(|| anyhow!("no [zerodha] section in config — add api_key and api_secret"))?;

    let mut kite_cfg = zerodha_cfg.into_venue_config();

    let login_url = kite_cfg
        .login_url()
        .ok_or_else(|| anyhow!("cannot build login URL — check api_key"))?;

    let redirect_url = format!("http://127.0.0.1:{port}");

    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║  Kite Connect Daily Login                                   ║");
    println!("╠══════════════════════════════════════════════════════════════╣");
    println!("║                                                             ║");
    println!("║  Redirect URL (set in Kite app):                            ║");
    println!("║     {redirect_url:<51} ║");
    println!("║                                                             ║");
    println!("║  Opening browser...                                         ║");
    println!("╚══════════════════════════════════════════════════════════════╝");
    println!();
    println!("  {login_url}");
    println!();

    if open_browser(&login_url).is_err() {
        println!("Could not open browser automatically — open the URL above manually.");
    }

    println!("Waiting for redirect on {redirect_url} ...");

    let rt = tokio::runtime::Runtime::new()?;
    let access_token = rt.block_on(run_callback_server(port, &mut kite_cfg))?;

    println!();
    println!("Access token obtained successfully.");
    println!("Token: {access_token}");
    println!();

    let token_path = format!("{config_path}.kite_token");
    let mut f =
        std::fs::File::create(&token_path).with_context(|| format!("creating {token_path}"))?;
    f.write_all(access_token.as_bytes())?;
    println!("Token saved to: {token_path}");
    println!("Valid until ~06:00 IST tomorrow. Re-run this command daily.");

    Ok(())
}

async fn run_callback_server(
    port: u16,
    cfg: &mut zooni::venues::zerodha::Config,
) -> Result<String> {
    use axum::extract::Query;
    use axum::response::Html;
    use std::collections::HashMap;
    use std::sync::Arc;
    use tokio::sync::oneshot;

    let (tx, rx) = oneshot::channel::<String>();
    let tx = Arc::new(tokio::sync::Mutex::new(Some(tx)));

    let app = axum::Router::new().route(
        "/",
        axum::routing::get({
            let tx = Arc::clone(&tx);
            move |Query(params): Query<HashMap<String, String>>| {
                let tx = Arc::clone(&tx);
                async move {
                    if let Some(token) = params.get("request_token") {
                        if let Some(sender) = tx.lock().await.take() {
                            let _ = sender.send(token.clone());
                        }
                        Html(
                            "<html><body><h2>Login successful!</h2>\
                             <p>You can close this tab. The token has been captured.</p>\
                             </body></html>"
                                .to_string(),
                        )
                    } else {
                        Html(
                            "<html><body><h2>Error</h2>\
                             <p>No request_token found in the redirect.</p>\
                             </body></html>"
                                .to_string(),
                        )
                    }
                }
            }
        }),
    );

    let listener = tokio::net::TcpListener::bind(format!("127.0.0.1:{port}"))
        .await
        .with_context(|| format!("binding to port {port}"))?;

    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });

    let request_token = rx.await.context("callback channel closed")?;
    server.abort();

    println!("Got request_token, exchanging for access_token...");

    let mut client = zooni::venues::zerodha::rest::Client::new(cfg.clone());
    let access_token = client.exchange_request_token(&request_token).await?;

    cfg.set_access_token(access_token.clone());

    Ok(access_token)
}
