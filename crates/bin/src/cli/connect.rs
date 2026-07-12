use crate::cli::api::ApiClient;
use crate::cli::config::ConnectOpts;
use crate::cli::tunnel::SshTunnel;
use crate::compat::CompatSession;

/// One live agent connection. Keep `tunnel` alive for as long as `api` is
/// used — dropping it closes the SSH forward.
pub struct AgentConn {
    pub tunnel: SshTunnel,
    pub api: ApiClient,
    pub compat: CompatSession,
}

/// The single entry point every agent-talking command goes through:
/// tunnel, client, `/v1/version` handshake, version-skew banners.
pub async fn connect_agent(connect: ConnectOpts) -> anyhow::Result<AgentConn> {
    let profile = connect.resolve()?;
    let tunnel = SshTunnel::open(&profile).await?;
    let api = ApiClient::new(tunnel.base_url.clone());
    let info = api.version().await?;
    let compat = CompatSession::new(env!("CARGO_PKG_VERSION"), &info);
    compat.emit_version_banners();
    Ok(AgentConn {
        tunnel,
        api,
        compat,
    })
}
