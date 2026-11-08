use std::sync::Arc;

use async_trait::async_trait;
use zendiloos_agent_proto::{
    AgentTransport, ConfirmWithin, ErrorCode, FirewallConfig, FirewallPortRange, FirewallPorts, FirewallRule,
    FirewallRules, FirewallSource, FirewallTraffic, FirewallVerdict, InterfaceName, Request, Response,
};
use zendiloos_firewall::application::{FirewallHost, HostError, HostState};
use zendiloos_firewall::domain::{Action, PortRange, Ruleset, Source, Traffic};
use zendiloos_kernel::{PortError, RequestContext, Timestamp};

use super::{WHAT, agent_error, call, call_raw, unexpected};

/// [`FirewallHost`] through the system agent.
pub struct AgentFirewall {
    transport: Arc<dyn AgentTransport>,
}

impl AgentFirewall {
    pub fn new(transport: Arc<dyn AgentTransport>) -> Self {
        Self { transport }
    }
}

fn invalid(what: &str) -> HostError {
    HostError::Port(PortError::new(WHAT, format!("not a valid {what} for the agent")))
}

fn verdict(action: Action) -> FirewallVerdict {
    match action {
        Action::Allow => FirewallVerdict::Accept,
        Action::Deny => FirewallVerdict::Drop,
    }
}

fn ports(ranges: &[PortRange]) -> Result<FirewallPorts, HostError> {
    let ranges = ranges
        .iter()
        .map(|r| FirewallPortRange::new(r.from(), r.to()).map_err(|_| invalid("port")))
        .collect::<Result<Vec<_>, _>>()?;
    FirewallPorts::new(ranges).map_err(|_| invalid("list of ports"))
}

fn source(source: Source) -> Result<FirewallSource, HostError> {
    match source {
        Source::Any => Ok(FirewallSource::Any),
        Source::Address(ip) => FirewallSource::network(ip, if ip.is_ipv4() { 32 } else { 128 }),
        Source::Network { address, prefix } => FirewallSource::network(address, prefix),
        Source::Range { from, to } => FirewallSource::range(from, to),
    }
    .map_err(|_| invalid("source address"))
}

/// The rules in the agent's terms: every value checked once more.
fn config(ruleset: &Ruleset) -> Result<FirewallConfig, HostError> {
    let rules = ruleset
        .rules
        .iter()
        .map(|rule| {
            Ok(FirewallRule {
                interface: rule
                    .interface
                    .as_deref()
                    .map(|name| InterfaceName::parse(name).map_err(|_| invalid("interface name")))
                    .transpose()?,
                source: source(rule.source)?,
                traffic: match &rule.traffic {
                    Traffic::All => FirewallTraffic::All {},
                    Traffic::Ping => FirewallTraffic::Ping {},
                    Traffic::Tcp(ranges) => FirewallTraffic::Tcp { ports: ports(ranges)? },
                    Traffic::Udp(ranges) => FirewallTraffic::Udp { ports: ports(ranges)? },
                },
                forward: rule.forward,
                verdict: verdict(rule.action),
            })
        })
        .collect::<Result<Vec<_>, HostError>>()?;
    Ok(FirewallConfig {
        rules: FirewallRules::new(rules).map_err(|_| invalid("number of rules"))?,
        fallback: verdict(ruleset.fallback),
    })
}

/// The agent's refusal or failure, in its words.
fn host_error(response: Response) -> HostError {
    match response {
        Response::Error { code: ErrorCode::Forbidden, message } => HostError::Refused(message),
        Response::Error { code: ErrorCode::Conflict, .. } => HostError::Conflict,
        Response::Error { code: ErrorCode::Failed, message } => HostError::Failed(message),
        Response::Error { code, message } => HostError::Port(agent_error(code, &message)),
        other => HostError::Port(unexpected(other)),
    }
}

#[async_trait]
impl FirewallHost for AgentFirewall {
    async fn state(&self, context: &RequestContext) -> Result<HostState, PortError> {
        match call(self.transport.as_ref(), Request::FirewallState {}, context).await? {
            Response::FirewallState { pending } => {
                Ok(HostState { pending_until: pending.map(|p| Timestamp::from_unix_seconds(p.revert_at)) })
            }
            other => Err(unexpected(other)),
        }
    }

    async fn apply(
        &self,
        ruleset: Option<&Ruleset>,
        confirm_within: Option<u16>,
        context: &RequestContext,
    ) -> Result<(), HostError> {
        let confirm_within = confirm_within
            .map(|seconds| ConfirmWithin::seconds(seconds).map_err(|_| invalid("time to confirm")))
            .transpose()?;
        let request = Request::ApplyFirewall { config: ruleset.map(config).transpose()?, confirm_within };
        match call_raw(self.transport.as_ref(), request, context).await? {
            Response::Done => Ok(()),
            other => Err(host_error(other)),
        }
    }

    async fn confirm(&self, context: &RequestContext) -> Result<(), HostError> {
        match call_raw(self.transport.as_ref(), Request::ConfirmFirewall {}, context).await? {
            Response::Done => Ok(()),
            other => Err(host_error(other)),
        }
    }
}

#[cfg(test)]
mod tests {
    use zendiloos_firewall::domain::{Compiled, parse_ports};
    use zendiloos_sim::SimulatedAgent;

    use super::*;

    fn compiled(interface: Option<&str>, source: Source, traffic: Traffic) -> Compiled {
        Compiled {
            interface: interface.map(str::to_owned),
            source,
            traffic,
            forward: true,
            action: Action::Allow,
        }
    }

    #[test]
    fn rules_become_the_agents_and_odd_ones_are_not_sent() {
        let ruleset = Ruleset {
            rules: vec![
                compiled(
                    Some("enp1s0"),
                    Source::parse("192.168.1.0/24").unwrap(),
                    Traffic::Tcp(parse_ports("445").unwrap()),
                ),
                compiled(None, Source::parse("10.0.0.1").unwrap(), Traffic::Udp(parse_ports("53").unwrap())),
                compiled(None, Source::parse("fd00::1-fd00::9").unwrap(), Traffic::Ping),
                compiled(None, Source::Any, Traffic::All),
            ],
            fallback: Action::Deny,
        };
        let made = config(&ruleset).unwrap();
        assert_eq!(made.fallback, FirewallVerdict::Drop);
        let rules: Vec<&FirewallRule> = made.rules.iter().collect();
        assert_eq!(rules[0].interface.as_ref().map(InterfaceName::as_str), Some("enp1s0"));
        assert_eq!(rules[1].source, FirewallSource::network("10.0.0.1".parse().unwrap(), 32).unwrap());
        assert!(matches!(rules[2].source, FirewallSource::Range { .. }));
        assert_eq!(
            (rules[3].traffic.clone(), rules[3].verdict),
            (FirewallTraffic::All {}, FirewallVerdict::Accept)
        );

        let odd =
            Ruleset { rules: vec![compiled(Some("lo"), Source::Any, Traffic::All)], fallback: Action::Allow };
        assert!(config(&odd).is_err());
        let backwards = Source::Range { from: "10.0.0.9".parse().unwrap(), to: "10.0.0.1".parse().unwrap() };
        assert!(source(backwards).is_err());
        assert!(ports(&[]).is_err());
    }

    #[tokio::test]
    async fn rules_are_applied_and_confirmed_through_the_agent() {
        let firewall = AgentFirewall::new(Arc::new(SimulatedAgent::new()));
        let context = RequestContext::default();
        let ruleset = Ruleset { rules: vec![], fallback: Action::Deny };
        assert_eq!(firewall.state(&context).await.unwrap().pending_until, None);
        firewall.apply(Some(&ruleset), Some(90), &context).await.unwrap();
        assert!(firewall.state(&context).await.unwrap().pending_until.is_some());
        assert!(matches!(firewall.apply(None, Some(90), &context).await, Err(HostError::Conflict)));
        firewall.apply(None, None, &context).await.unwrap();
        firewall.confirm(&context).await.unwrap();
        assert!(matches!(firewall.confirm(&context).await, Err(HostError::Refused(_))));
        assert!(matches!(firewall.apply(None, Some(5), &context).await, Err(HostError::Port(_))));
        let failed = host_error(Response::Error { code: ErrorCode::Failed, message: "nft: no".into() });
        assert!(matches!(failed, HostError::Failed(reason) if reason == "nft: no"));
        assert!(matches!(host_error(Response::Done), HostError::Port(_)));
        let odd = host_error(Response::Error { code: ErrorCode::InvalidRequest, message: "x".into() });
        assert!(matches!(odd, HostError::Port(_)));
    }
}
