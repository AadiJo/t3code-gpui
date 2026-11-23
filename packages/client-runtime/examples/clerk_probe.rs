//! Drives the native Clerk sign-in against the production Clerk instance as
//! far as it goes without an account: restore with no session, then start an
//! email-code sign-in for an address on the reserved `example.com` domain,
//! which Clerk answers with "account not found" before sending any email.
//!
//! Usage: `clerk_probe <out_dir>`; writes `<out_dir>/clerk-probe.txt`.

use std::sync::Arc;

use t3code_client_runtime::platform::FileSecretStore;
use t3code_client_runtime::relay::{ClerkError, ClerkSession, CloudConfig};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let out = std::path::PathBuf::from(std::env::args().nth(1).expect("usage: clerk_probe <out_dir>"));
    std::fs::create_dir_all(&out)?;
    let secrets_path = out.join("clerk-probe-secrets.json");
    _ = std::fs::remove_file(&secrets_path);
    let secrets = Arc::new(FileSecretStore::new(secrets_path.clone()));
    let mut log = String::new();
    let config = CloudConfig::production();
    log += &format!(
        "frontend api host: {:?}\n",
        t3code_client_runtime::relay::clerk::frontend_api_host(&config.clerk_publishable_key)
    );
    let session = ClerkSession::new(&config, secrets)?;
    log += &format!("account before restore: {:?}\n", *session.account().borrow());
    session.restore().await?;
    log += &format!("account after restore with no stored client: {:?}\n", *session.account().borrow());
    let email = "nobody-t3native-probe@example.com";
    match session.start_email_code(email).await {
        Ok(sign_in) => log += &format!("UNEXPECTED: Clerk started a sign-in {sign_in:?}\n"),
        Err(ClerkError::Rejected { code, message }) => {
            log += &format!("start_email_code({email}) -> rejected: code={code} message={message:?}\n")
        }
        Err(error) => log += &format!("start_email_code({email}) -> error: {error}\n"),
    }
    let stored = std::fs::read_to_string(&secrets_path).unwrap_or_default();
    log += &format!(
        "native client JWT stored after the first FAPI call: {}\n",
        stored.contains("clerk-client-jwt")
    );
    log += &format!("token while signed out: {:?}\n", session.template_token().await?);
    _ = std::fs::remove_file(&secrets_path);
    print!("{log}");
    std::fs::write(out.join("clerk-probe.txt"), log)?;
    Ok(())
}
