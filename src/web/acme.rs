use instant_acme::{
    Account,
    AccountCredentials,
    ChallengeType,
    Identifier,
    LetsEncrypt,
    NewAccount,
    NewOrder,
    OrderStatus,
    RetryPolicy,
};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

pub async fn ensure_certificate(
    domain: &str,
    email: &str,
    http_port: u16,
    cert_path: &Path,
    key_path: &Path,
    account_path: &Path,
) -> anyhow::Result<bool> {
    if cert_path.exists()
        && let Ok(meta) = std::fs::metadata(cert_path)
        && let Ok(modified) = meta.modified()
        && let Ok(age) = modified.elapsed()
        && age < Duration::from_secs(60 * 24 * 3600)
    {
        log::debug!(
            "[ACME] Certificate at {} is less than 60 days old — skipping renewal",
            cert_path.display()
        );
        return Ok(false);
    }
    log::info!("[ACME] Starting certificate issuance/renewal for {}", domain);
    let account = if account_path.exists() {
        let json = std::fs::read_to_string(account_path)?;
        let creds: AccountCredentials = serde_json::from_str(&json)?;
        log::debug!("[ACME] Loaded credentials from {}", account_path.display());
        Account::builder()?.from_credentials(creds).await?
    } else {
        let contact = format!("mailto:{}", email);
        log::info!("[ACME] Creating new ACME account for {}", email);
        let (account, creds) = Account::builder()?
            .create(
                &NewAccount {
                    contact: &[&contact],
                    terms_of_service_agreed: true,
                    only_return_existing: false,
                },
                LetsEncrypt::Production.url().to_string(),
                None,
            )
            .await?;
        let json = serde_json::to_string(&creds)?;
        std::fs::write(account_path, &json)?;
        log::info!("[ACME] Account credentials saved to {}", account_path.display());
        account
    };
    let identifier = Identifier::Dns(domain.to_string());
    let mut order = account.new_order(&NewOrder::new(&[identifier])).await?;
    let (server_handle, challenge_thread) = {
        let mut auths = order.authorizations();
        let mut auth_handle = auths
            .next()
            .await
            .ok_or_else(|| anyhow::anyhow!("[ACME] No authorizations returned"))??;
        let token: String = auth_handle
            .challenges
            .iter()
            .find(|c| c.r#type == ChallengeType::Http01)
            .ok_or_else(|| anyhow::anyhow!("[ACME] No HTTP-01 challenge in authorization"))?
            .token
            .clone();
        let mut challenge_handle = auth_handle
            .challenge(ChallengeType::Http01)
            .ok_or_else(|| anyhow::anyhow!("[ACME] Could not get HTTP-01 challenge handle"))?;
        let key_auth: String = challenge_handle.key_authorization().as_str().to_string();
        let (handle_tx, handle_rx) =
            std::sync::mpsc::sync_channel::<actix_web::dev::ServerHandle>(1);
        let token_arc = Arc::new(token);
        let key_auth_arc = Arc::new(key_auth);
        let token_for_thread = Arc::clone(&token_arc);
        let key_auth_for_thread = Arc::clone(&key_auth_arc);
        let challenge_thread = std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("failed to build ACME challenge server runtime");
            rt.block_on(async move {
                let tok = token_for_thread;
                let kauth = key_auth_for_thread;
                let server = actix_web::HttpServer::new(move || {
                    let tok = Arc::clone(&tok);
                    let kauth = Arc::clone(&kauth);
                    actix_web::App::new().route(
                        "/.well-known/acme-challenge/{token}",
                        actix_web::web::get().to(
                            move |path: actix_web::web::Path<String>| {
                                let tok = Arc::clone(&tok);
                                let kauth = Arc::clone(&kauth);
                                async move {
                                    if path.as_str() == tok.as_str() {
                                        actix_web::HttpResponse::Ok()
                                            .content_type("text/plain")
                                            .body(kauth.as_ref().clone())
                                    } else {
                                        actix_web::HttpResponse::NotFound().finish()
                                    }
                                }
                            },
                        ),
                    )
                })
                .bind(format!("0.0.0.0:{}", http_port))
                .unwrap()
                .run();
                let _ = handle_tx.send(server.handle());
                let _ = server.await;
            });
        });
        let server_handle = tokio::task::spawn_blocking(move || handle_rx.recv().ok())
            .await
            .ok()
            .flatten()
            .ok_or_else(|| {
                anyhow::anyhow!("[ACME] Challenge server failed to bind on port {}", http_port)
            })?;
        log::info!("[ACME] Challenge server running on port {}", http_port);
        challenge_handle.set_ready().await?;
        (server_handle, challenge_thread)
    };
    let retry = RetryPolicy::new()
        .initial_delay(Duration::from_secs(3))
        .timeout(Duration::from_secs(120));
    let status = order.poll_ready(&retry).await?;
    server_handle.stop(true).await;
    let _ = tokio::task::spawn_blocking(move || {
        let _ = challenge_thread.join();
    })
    .await;
    if status == OrderStatus::Invalid {
        return Err(anyhow::anyhow!(
            "[ACME] Order became Invalid during challenge validation"
        ));
    }
    let private_key_pem = order.finalize().await?;
    let cert_chain = order.poll_certificate(&retry).await?;
    std::fs::write(cert_path, cert_chain.as_bytes())?;
    std::fs::write(key_path, private_key_pem.as_bytes())?;
    log::info!(
        "[ACME] Certificate written to {} / {}",
        cert_path.display(),
        key_path.display()
    );
    Ok(true)
}