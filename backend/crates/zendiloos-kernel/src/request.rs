use std::net::IpAddr;

/// Where a request came from. Use cases need it for the audit log and for
/// throttling; the primary adapter fills it in.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RequestContext {
    pub client_ip: Option<IpAddr>,
    pub user_agent: Option<String>,
    /// Correlates the audit log with the server and agent logs.
    pub request_id: Option<String>,
}
