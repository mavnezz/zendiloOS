//! Certificates from Let's Encrypt, made by acme.sh as OPNsense's ACME
//! client makes them: accounts with the authority, the ways a domain is
//! proven (a DNS record through a provider's API, or a file served over
//! HTTP), certificates for one or more names, renewed before they expire.
//! And the web interface's ports and the certificate it shows.

use std::collections::{BTreeMap, BTreeSet};

use zendiloos_kernel::Timestamp;

/// The authority certificates come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcmeServer {
    LetsEncrypt,
    /// Let's Encrypt's test service: certificates no browser trusts, with
    /// limits generous enough to try a setup out.
    LetsEncryptStaging,
}

impl AcmeServer {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::LetsEncrypt => "letsencrypt",
            Self::LetsEncryptStaging => "letsencrypt_staging",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "letsencrypt" => Some(Self::LetsEncrypt),
            "letsencrypt_staging" => Some(Self::LetsEncryptStaging),
            _ => None,
        }
    }
}

/// An account with the authority: who it writes to about the certificates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcmeAccount {
    pub id: i64,
    pub name: String,
    pub server: AcmeServer,
    pub email: String,
    pub created_at: Timestamp,
}

/// An account as given; checked by [`NewAccount::check`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewAccount {
    pub name: String,
    pub server: AcmeServer,
    pub email: String,
}

/// Why something given cannot be taken, in words for the person who gave it.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct Invalid(pub String);

fn invalid(text: impl Into<String>) -> Invalid {
    Invalid(text.into())
}

/// A name of one line, 1 to 64 characters.
fn check_name(name: &str) -> Result<String, Invalid> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 64 || name.chars().any(char::is_control) {
        return Err(invalid("a name has 1 to 64 characters on one line"));
    }
    Ok(name.to_owned())
}

/// An address the authority accepts, as the system agent hands it to
/// acme.sh: a local part of letters, digits and `._%+-` (not starting with
/// `-`), one `@`, a domain of lowercase letters, digits and `-` in labels
/// with a dot between, at most 254 characters.
pub fn is_email(email: &str) -> bool {
    let Some((local, domain)) = email.split_once('@') else { return false };
    let label = |label: &str| {
        (1..=63).contains(&label.len())
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    };
    email.len() <= 254
        && !local.is_empty()
        && !local.starts_with('-')
        && local.bytes().all(|b| b.is_ascii_alphanumeric() || b"._%+-".contains(&b))
        && domain.contains('.')
        && domain.split('.').all(label)
        && !domain.rsplit('.').next().is_some_and(|tld| tld.bytes().all(|b| b.is_ascii_digit()))
}

/// `email` without spaces around it and with its domain in lowercase, as
/// domains are whatever their case: what is stored and given to acme.sh.
pub fn normalize_email(email: &str) -> String {
    let email = email.trim();
    match email.rsplit_once('@') {
        Some((local, domain)) => format!("{local}@{}", domain.to_ascii_lowercase()),
        None => email.to_owned(),
    }
}

impl NewAccount {
    pub fn check(self) -> Result<Self, Invalid> {
        let email = normalize_email(&self.email);
        if !is_email(&email) {
            return Err(invalid("not an e-mail address the authority would take"));
        }
        Ok(Self { name: check_name(&self.name)?, server: self.server, email })
    }
}

/// A value a DNS provider's API needs, by the name acme.sh reads it under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DnsVariable {
    pub name: &'static str,
    /// Never shown again once saved.
    pub secret: bool,
    pub required: bool,
    /// What a value looks like, or what the script takes without one: shown
    /// in the empty field.
    pub hint: Option<&'static str>,
}

/// A DNS provider whose API acme.sh speaks (its `dnsapi` scripts in
/// Debian's package), with the values it reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DnsProvider {
    /// The script's name, `dns_ionos`.
    pub api: &'static str,
    pub label: &'static str,
    pub variables: &'static [DnsVariable],
}

const fn var(name: &'static str, secret: bool, required: bool) -> DnsVariable {
    DnsVariable { name, secret, required, hint: None }
}

/// A value, with what it looks like.
const fn hinted(name: &'static str, secret: bool, required: bool, hint: &'static str) -> DnsVariable {
    DnsVariable { name, secret, required, hint: Some(hint) }
}

/// A provider whose API takes one secret: a token or a key.
const fn token(api: &'static str, label: &'static str, variables: &'static [DnsVariable]) -> DnsProvider {
    DnsProvider { api, label, variables }
}

/// The providers offered, each checked against acme.sh 3.1.1 as Debian 13
/// ships it: the names its scripts read.
pub const DNS_PROVIDERS: &[DnsProvider] = &[
    DnsProvider {
        api: "dns_ionos",
        label: "IONOS",
        variables: &[var("IONOS_PREFIX", false, true), var("IONOS_SECRET", true, true)],
    },
    DnsProvider {
        api: "dns_cf",
        label: "Cloudflare",
        variables: &[
            var("CF_Token", true, true),
            var("CF_Account_ID", false, false),
            var("CF_Zone_ID", false, false),
        ],
    },
    DnsProvider { api: "dns_hetzner", label: "Hetzner DNS", variables: &[var("HETZNER_Token", true, true)] },
    DnsProvider { api: "dns_desec", label: "deSEC", variables: &[var("DEDYN_TOKEN", true, true)] },
    DnsProvider {
        api: "dns_inwx",
        label: "INWX",
        variables: &[
            var("INWX_User", false, true),
            var("INWX_Password", true, true),
            var("INWX_Shared_Secret", true, false),
        ],
    },
    DnsProvider {
        api: "dns_netcup",
        label: "netcup",
        variables: &[var("NC_CID", false, true), var("NC_Apikey", true, true), var("NC_Apipw", true, true)],
    },
    DnsProvider {
        api: "dns_ovh",
        label: "OVHcloud",
        variables: &[
            var("OVH_END_POINT", false, true),
            var("OVH_AK", false, true),
            var("OVH_AS", true, true),
            var("OVH_CK", true, true),
        ],
    },
    DnsProvider {
        api: "dns_gandi_livedns",
        label: "Gandi LiveDNS",
        variables: &[var("GANDI_LIVEDNS_TOKEN", true, true)],
    },
    DnsProvider {
        api: "dns_porkbun",
        label: "Porkbun",
        variables: &[var("PORKBUN_API_KEY", true, true), var("PORKBUN_SECRET_API_KEY", true, true)],
    },
    DnsProvider {
        api: "dns_aws",
        label: "Amazon Route 53",
        variables: &[var("AWS_ACCESS_KEY_ID", false, true), var("AWS_SECRET_ACCESS_KEY", true, true)],
    },
    DnsProvider {
        api: "dns_he",
        label: "Hurricane Electric",
        variables: &[var("HE_Username", false, true), var("HE_Password", true, true)],
    },
    DnsProvider { api: "dns_duckdns", label: "Duck DNS", variables: &[var("DuckDNS_Token", true, true)] },
    DnsProvider { api: "dns_dynv6", label: "dynv6", variables: &[var("DYNV6_TOKEN", true, true)] },
    DnsProvider { api: "dns_ipv64", label: "IPv64.net", variables: &[var("IPv64_Token", true, true)] },
    // Dynamic DNS for a home connection.
    token("dns_ddnss", "DDNSS.de", &[var("DDNSS_Token", true, true)]),
    DnsProvider {
        api: "dns_dnshome",
        label: "dnsHome.de",
        variables: &[var("DNSHOME_Subdomain", false, true), var("DNSHOME_SubdomainPassword", true, true)],
    },
    DnsProvider {
        api: "dns_df",
        label: "DynDnsFree.de",
        variables: &[var("DF_user", false, true), var("DF_password", true, true)],
    },
    DnsProvider {
        api: "dns_selfhost",
        label: "SelfHost.de",
        variables: &[
            var("SELFHOSTDNS_USERNAME", false, true),
            var("SELFHOSTDNS_PASSWORD", true, true),
            hinted("SELFHOSTDNS_MAP", false, true, "_acme-challenge.example.com:12345"),
        ],
    },
    DnsProvider {
        api: "dns_freedns",
        label: "FreeDNS (afraid.org)",
        variables: &[var("FREEDNS_User", false, true), var("FREEDNS_Password", true, true)],
    },
    DnsProvider {
        api: "dns_dynu",
        label: "Dynu",
        variables: &[var("Dynu_ClientId", false, true), var("Dynu_Secret", true, true)],
    },
    token("dns_freemyip", "FreeMyIP", &[var("FREEMYIP_Token", true, true)]),
    DnsProvider {
        api: "dns_dnsexit",
        label: "DNSExit",
        variables: &[
            var("DNSEXIT_API_KEY", true, true),
            var("DNSEXIT_AUTH_USER", false, true),
            var("DNSEXIT_AUTH_PASS", true, true),
        ],
    },
    DnsProvider {
        api: "dns_zoneedit",
        label: "ZoneEdit",
        variables: &[var("ZONEEDIT_ID", false, true), var("ZONEEDIT_Token", true, true)],
    },
    token("dns_he_ddns", "Hurricane Electric (DDNS key)", &[var("HE_DDNS_KEY", true, true)]),
    // Hosting and domains in Germany, Austria and Switzerland.
    DnsProvider {
        api: "dns_kas",
        label: "All-Inkl (KAS)",
        variables: &[
            var("KAS_Login", false, true),
            hinted("KAS_Authtype", false, true, "plain"),
            var("KAS_Authdata", true, true),
        ],
    },
    DnsProvider {
        api: "dns_hostingde",
        label: "Hosting.de",
        variables: &[
            hinted("HOSTINGDE_ENDPOINT", false, true, "https://secure.hosting.de"),
            var("HOSTINGDE_APIKEY", true, true),
        ],
    },
    token("dns_ionos_cloud", "IONOS Cloud DNS", &[var("IONOS_TOKEN", true, true)]),
    token("dns_variomedia", "Variomedia", &[var("VARIOMEDIA_API_TOKEN", true, true)]),
    DnsProvider {
        api: "dns_cn",
        label: "Core-Networks",
        variables: &[var("CN_User", false, true), var("CN_Password", true, true)],
    },
    DnsProvider {
        api: "dns_artfiles",
        label: "ArtFiles",
        variables: &[var("AF_API_USERNAME", false, true), var("AF_API_PASSWORD", true, true)],
    },
    DnsProvider {
        api: "dns_schlundtech",
        label: "SchlundTech",
        variables: &[var("SCHLUNDTECH_USER", false, true), var("SCHLUNDTECH_PASSWORD", true, true)],
    },
    DnsProvider {
        api: "dns_udr",
        label: "united-domains (Reselling)",
        variables: &[var("UDR_USER", false, true), var("UDR_PASS", true, true)],
    },
    token("dns_doapi", "Domain-Offensive (do.de)", &[var("DO_LETOKEN", true, true)]),
    token("dns_limacity", "lima-city", &[var("LIMACITY_APIKEY", true, true)]),
    DnsProvider {
        api: "dns_servercow",
        label: "ServerCow",
        variables: &[var("SERVERCOW_API_Username", false, true), var("SERVERCOW_API_Password", true, true)],
    },
    DnsProvider {
        api: "dns_nm",
        label: "NameMaster",
        variables: &[
            var("NM_user", false, true),
            hinted("NM_sha256", true, true, "SHA-256 of the API password"),
        ],
    },
    DnsProvider {
        api: "dns_autodns",
        label: "InterNetX autoDNS",
        variables: &[
            var("AUTODNS_USER", false, true),
            var("AUTODNS_PASSWORD", true, true),
            hinted("AUTODNS_CONTEXT", false, true, "4"),
        ],
    },
    DnsProvider {
        api: "dns_euserv",
        label: "EUserv",
        variables: &[var("EUSERV_Username", false, true), var("EUSERV_Password", true, true)],
    },
    DnsProvider {
        api: "dns_world4you",
        label: "World4You",
        variables: &[var("WORLD4YOU_USERNAME", false, true), var("WORLD4YOU_PASSWORD", true, true)],
    },
    DnsProvider {
        api: "dns_kappernet",
        label: "kapper.net",
        variables: &[var("KAPPERNETDNS_Key", false, true), var("KAPPERNETDNS_Secret", true, true)],
    },
    DnsProvider {
        api: "dns_rcode0",
        label: "RcodeZero",
        variables: &[
            var("RCODE0_API_TOKEN", true, true),
            hinted("RCODE0_URL", false, false, "https://my.rcodezero.at"),
            hinted("RCODE0_TTL", false, false, "60"),
        ],
    },
    DnsProvider {
        api: "dns_cyon",
        label: "cyon",
        variables: &[
            var("CY_Username", false, true),
            var("CY_Password", true, true),
            hinted("CY_OTP_Secret", true, false, "only with two-factor sign-in"),
        ],
    },
    token("dns_infomaniak", "Infomaniak", &[var("INFOMANIAK_API_TOKEN", true, true)]),
    // Registrars and clouds.
    DnsProvider {
        api: "dns_gd",
        label: "GoDaddy",
        variables: &[var("GD_Key", false, true), var("GD_Secret", true, true)],
    },
    DnsProvider {
        api: "dns_namecheap",
        label: "Namecheap",
        variables: &[
            var("NAMECHEAP_USERNAME", false, true),
            var("NAMECHEAP_API_KEY", true, true),
            hinted("NAMECHEAP_SOURCEIP", false, true, "the NAS's public IP address, allowed at Namecheap"),
        ],
    },
    DnsProvider {
        api: "dns_namecom",
        label: "Name.com",
        variables: &[var("Namecom_Username", false, true), var("Namecom_Token", true, true)],
    },
    token("dns_namesilo", "NameSilo", &[var("Namesilo_Key", true, true)]),
    token("dns_dgon", "DigitalOcean", &[var("DO_API_KEY", true, true)]),
    token("dns_linode_v4", "Linode (Akamai)", &[var("LINODE_V4_API_KEY", true, true)]),
    token("dns_vultr", "Vultr", &[var("VULTR_API_KEY", true, true)]),
    token("dns_scaleway", "Scaleway", &[var("SCALEWAY_API_TOKEN", true, true)]),
    DnsProvider {
        api: "dns_azure",
        label: "Azure DNS",
        variables: &[
            var("AZUREDNS_SUBSCRIPTIONID", false, true),
            var("AZUREDNS_TENANTID", false, true),
            var("AZUREDNS_APPID", false, true),
            var("AZUREDNS_CLIENTSECRET", true, true),
        ],
    },
    token("dns_bunny", "Bunny.net", &[var("BUNNY_API_KEY", true, true)]),
    token("dns_gcore", "Gcore", &[var("GCORE_Key", true, true)]),
    token("dns_dnsimple", "DNSimple", &[var("DNSimple_OAUTH_TOKEN", true, true)]),
    DnsProvider {
        api: "dns_cloudns",
        label: "ClouDNS",
        variables: &[
            hinted("CLOUDNS_AUTH_ID", false, false, "or the sub auth ID below"),
            var("CLOUDNS_SUB_AUTH_ID", false, false),
            var("CLOUDNS_AUTH_PASSWORD", true, true),
        ],
    },
    DnsProvider {
        api: "dns_one",
        label: "one.com",
        variables: &[var("ONECOM_User", false, true), var("ONECOM_Password", true, true)],
    },
    token("dns_netlify", "Netlify", &[var("NETLIFY_ACCESS_TOKEN", true, true)]),
    token("dns_vercel", "Vercel", &[var("VERCEL_TOKEN", true, true)]),
    token("dns_njalla", "Njalla", &[var("NJALLA_Token", true, true)]),
    token("dns_mijnhost", "mijn.host", &[var("MIJNHOST_API_KEY", true, true)]),
    DnsProvider {
        api: "dns_openprovider",
        label: "Openprovider",
        variables: &[var("OPENPROVIDER_USER", false, true), var("OPENPROVIDER_PASSWORDHASH", true, true)],
    },
    DnsProvider {
        api: "dns_joker",
        label: "Joker.com",
        variables: &[var("JOKER_USERNAME", false, true), var("JOKER_PASSWORD", true, true)],
    },
    DnsProvider {
        api: "dns_loopia",
        label: "Loopia",
        variables: &[
            var("LOOPIA_User", false, true),
            var("LOOPIA_Password", true, true),
            hinted("LOOPIA_Api", false, false, "https://api.loopia.se/RPCSERV"),
        ],
    },
    DnsProvider {
        api: "dns_simply",
        label: "Simply.com",
        variables: &[var("SIMPLY_AccountName", false, true), var("SIMPLY_ApiKey", true, true)],
    },
    DnsProvider {
        api: "dns_exoscale",
        label: "Exoscale",
        variables: &[var("EXOSCALE_API_KEY", false, true), var("EXOSCALE_SECRET_KEY", true, true)],
    },
    token("dns_leaseweb", "Leaseweb", &[var("LSW_Key", true, true)]),
    DnsProvider {
        api: "dns_easydns",
        label: "easyDNS",
        variables: &[var("EASYDNS_Token", false, true), var("EASYDNS_Key", true, true)],
    },
    DnsProvider {
        api: "dns_me",
        label: "DNS Made Easy",
        variables: &[var("ME_Key", false, true), var("ME_Secret", true, true)],
    },
    token("dns_nsone", "NS1", &[var("NS1_Key", true, true)]),
    DnsProvider {
        api: "dns_ultra",
        label: "UltraDNS",
        variables: &[var("ULTRA_USR", false, true), var("ULTRA_PWD", true, true)],
    },
    DnsProvider {
        api: "dns_edgedns",
        label: "Akamai Edge DNS",
        variables: &[
            hinted("AKAMAI_HOST", false, true, "akab-….luna.akamaiapis.net"),
            var("AKAMAI_ACCESS_TOKEN", true, true),
            var("AKAMAI_CLIENT_TOKEN", true, true),
            var("AKAMAI_CLIENT_SECRET", true, true),
        ],
    },
    // A DNS server or panel of one's own.
    DnsProvider {
        api: "dns_pdns",
        label: "PowerDNS",
        variables: &[
            hinted("PDNS_Url", false, true, "http://ns.example.com:8081"),
            hinted("PDNS_ServerId", false, true, "localhost"),
            var("PDNS_Token", true, true),
            hinted("PDNS_Ttl", false, false, "60"),
        ],
    },
    DnsProvider {
        api: "dns_technitium",
        label: "Technitium DNS Server",
        variables: &[
            hinted("Technitium_Server", false, true, "https://dns.example.com:53443"),
            var("Technitium_Token", true, true),
        ],
    },
    DnsProvider {
        api: "dns_acmedns",
        label: "acme-dns",
        variables: &[
            hinted("ACMEDNS_BASE_URL", false, false, "https://auth.acme-dns.io"),
            var("ACMEDNS_USERNAME", false, false),
            var("ACMEDNS_PASSWORD", true, false),
            var("ACMEDNS_SUBDOMAIN", false, false),
        ],
    },
    DnsProvider {
        api: "dns_opnsense",
        label: "OPNsense",
        variables: &[
            hinted("OPNs_Host", false, true, "opnsense.example.com"),
            hinted("OPNs_Port", false, false, "443"),
            var("OPNs_Key", false, true),
            var("OPNs_Token", true, true),
            hinted("OPNs_Api_Insecure", false, false, "0, or 1 to accept any certificate"),
        ],
    },
    DnsProvider {
        api: "dns_miab",
        label: "Mail-in-a-Box",
        variables: &[
            hinted("MIAB_Server", false, true, "box.example.com"),
            var("MIAB_Username", false, true),
            var("MIAB_Password", true, true),
        ],
    },
    DnsProvider {
        api: "dns_cpanel",
        label: "cPanel",
        variables: &[
            hinted("cPanel_Hostname", false, true, "https://cpanel.example.com:2083"),
            var("cPanel_Username", false, true),
            var("cPanel_Apitoken", true, true),
        ],
    },
    DnsProvider {
        api: "dns_pleskxml",
        label: "Plesk",
        variables: &[
            hinted(
                "pleskxml_uri",
                false,
                true,
                "https://plesk.example.com:8443/enterprise/control/agent.php",
            ),
            var("pleskxml_user", false, true),
            var("pleskxml_pass", true, true),
        ],
    },
    DnsProvider {
        api: "dns_ispconfig",
        label: "ISPConfig",
        variables: &[
            hinted("ISPC_Api", false, true, "https://ispc.example.com:8080/remote/json.php"),
            var("ISPC_User", false, true),
            var("ISPC_Password", true, true),
            hinted("ISPC_Api_Insecure", false, false, "0, or 1 to accept any certificate"),
        ],
    },
    DnsProvider {
        api: "dns_da",
        label: "DirectAdmin",
        variables: &[
            hinted("DA_Api", true, true, "https://user:password@da.example.com:2222"),
            hinted("DA_Api_Insecure", false, false, "0, or 1 to accept any certificate"),
        ],
    },
];

pub fn dns_provider(api: &str) -> Option<&'static DnsProvider> {
    DNS_PROVIDERS.iter().find(|provider| provider.api == api)
}

/// Longest value a provider's API is given.
pub const MAX_VALUE: usize = 4096;

/// How the authority is shown that the names are ours.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChallengeMethod {
    /// A TXT record, set through the provider's API: works for a NAS no one
    /// reaches from outside, and the only way to a wildcard.
    Dns { provider: String, values: BTreeMap<String, String> },
    /// A file the authority fetches over HTTP on port 80.
    Http,
}

impl ChallengeMethod {
    /// Checks a method as given, taking from `before` the secret values left
    /// empty (they are never shown again, so an empty one means unchanged).
    pub fn check(self, before: Option<&ChallengeMethod>) -> Result<Self, Invalid> {
        let ChallengeMethod::Dns { provider, values } = self else { return Ok(Self::Http) };
        let known = dns_provider(&provider).ok_or_else(|| invalid("not a DNS provider offered here"))?;
        let earlier = match before {
            Some(ChallengeMethod::Dns { provider: was, values }) if *was == provider => Some(values),
            _ => None,
        };
        if let Some(name) = values.keys().find(|name| !known.variables.iter().any(|v| v.name == *name)) {
            return Err(invalid(format!("{} reads no value {name}", known.label)));
        }
        let mut kept = BTreeMap::new();
        for variable in known.variables {
            let given = values.get(variable.name).map(|v| v.trim().to_owned()).unwrap_or_default();
            let value = if given.is_empty() && variable.secret {
                earlier.and_then(|e| e.get(variable.name).cloned()).unwrap_or_default()
            } else {
                given
            };
            if value.len() > MAX_VALUE || value.chars().any(char::is_control) {
                return Err(invalid(format!("{} is too long or not on one line", variable.name)));
            }
            if value.is_empty() {
                if variable.required {
                    return Err(invalid(format!("{} needs {}", known.label, variable.name)));
                }
                continue;
            }
            kept.insert(variable.name.to_owned(), value);
        }
        Ok(Self::Dns { provider, values: kept })
    }

    pub fn is_dns(&self) -> bool {
        matches!(self, Self::Dns { .. })
    }
}

/// A way of proving names, kept to be used by certificates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Challenge {
    pub id: i64,
    pub name: String,
    pub method: ChallengeMethod,
    pub created_at: Timestamp,
}

pub fn check_challenge_name(name: &str) -> Result<String, Invalid> {
    check_name(name)
}

/// The kind of key a certificate gets.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum KeyType {
    /// ECDSA on P-256: small and fast, what every current client takes.
    #[default]
    Ec256,
    Rsa2048,
    Rsa4096,
}

impl KeyType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ec256 => "ec256",
            Self::Rsa2048 => "rsa2048",
            Self::Rsa4096 => "rsa4096",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        [Self::Ec256, Self::Rsa2048, Self::Rsa4096].into_iter().find(|key| key.as_str() == text)
    }
}

/// Most names one certificate carries.
pub const MAX_NAMES: usize = 50;

/// A host name as the authority takes it: letters, digits and hyphens in
/// labels of 1 to 63, at least two labels, at most 253 characters; with
/// `*.` in front for a wildcard. Lowercase.
pub fn is_domain(name: &str) -> bool {
    let bare = name.strip_prefix("*.").unwrap_or(name);
    let labels: Vec<&str> = bare.split('.').collect();
    name.len() <= 253
        && labels.len() >= 2
        && labels.iter().all(|label| {
            (1..=63).contains(&label.len())
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
        && !labels.last().is_some_and(|tld| tld.bytes().all(|b| b.is_ascii_digit()))
}

/// The names of a certificate as given, one per entry: trimmed and in
/// lowercase, the first its common name; none twice.
pub fn check_domains(given: &[String]) -> Result<Vec<String>, Invalid> {
    let names: Vec<String> =
        given.iter().map(|name| name.trim().to_ascii_lowercase()).filter(|name| !name.is_empty()).collect();
    if names.is_empty() {
        return Err(invalid("a certificate needs at least one name"));
    }
    if names.len() > MAX_NAMES {
        return Err(invalid(format!("at most {MAX_NAMES} names")));
    }
    if let Some(name) = names.iter().find(|name| !is_domain(name)) {
        return Err(invalid(format!("{name} is no host name such as nas.example.com or *.example.com")));
    }
    let mut seen = BTreeSet::new();
    if let Some(name) = names.iter().find(|name| !seen.insert(name.as_str())) {
        return Err(invalid(format!("{name} is given twice")));
    }
    Ok(names)
}

/// A certificate as given; checked by [`NewCertificate::check`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewCertificate {
    pub domains: Vec<String>,
    pub key: KeyType,
    pub account_id: i64,
    pub challenge_id: i64,
    pub auto_renew: bool,
}

impl NewCertificate {
    /// Checks the names; a wildcard only with a DNS challenge, which is the
    /// only one the authority proves it with.
    pub fn check(self, method: &ChallengeMethod) -> Result<Self, Invalid> {
        let domains = check_domains(&self.domains)?;
        if !method.is_dns() && domains.iter().any(|name| name.starts_with("*.")) {
            return Err(invalid("a wildcard needs a DNS challenge"));
        }
        Ok(Self { domains, ..self })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Certificate {
    pub id: i64,
    /// The first is its common name.
    pub domains: Vec<String>,
    pub key: KeyType,
    pub account_id: i64,
    pub challenge_id: i64,
    pub auto_renew: bool,
    pub created_at: Timestamp,
    /// When it was last issued, and until when that one holds.
    pub issued_at: Option<Timestamp>,
    pub not_after: Option<Timestamp>,
    /// The last try to issue or renew it, and what went wrong then.
    pub last_attempt_at: Option<Timestamp>,
    pub last_error: Option<String>,
    /// What acme.sh wrote the last time.
    pub last_output: Vec<String>,
    /// Whether nginx took the pair last issued, where the web interface
    /// shows it (or a change waiting would); `false`: its reload failed and
    /// nginx serves the one before, until the daily look has it take it.
    pub served: bool,
}

/// A certificate is renewed this long before it expires: Let's Encrypt's
/// hold 90 days, so two months after each renewal.
pub const RENEW_BEFORE_SECONDS: i64 = 30 * 24 * 3600;
/// A failed renewal is tried again after this long.
pub const RETRY_AFTER_SECONDS: i64 = 20 * 3600;

impl Certificate {
    pub fn common_name(&self) -> &str {
        self.domains.first().map_or("", String::as_str)
    }

    /// Issued and not expired at `now`.
    pub fn is_valid(&self, now: Timestamp) -> bool {
        self.issued_at.is_some() && self.not_after.is_some_and(|until| until > now)
    }

    /// To be renewed now: renewing is on, it was issued, it expires within
    /// [`RENEW_BEFORE_SECONDS`], and no try failed in the last
    /// [`RETRY_AFTER_SECONDS`]. One never issued is issued by hand first.
    pub fn renewal_due(&self, now: Timestamp) -> bool {
        let (Some(_), Some(until)) = (self.issued_at, self.not_after) else { return false };
        let failed_lately = self.last_error.is_some()
            && self
                .last_attempt_at
                .is_some_and(|at| now.unix_seconds() - at.unix_seconds() < RETRY_AFTER_SECONDS);
        self.auto_renew && until.unix_seconds() - now.unix_seconds() <= RENEW_BEFORE_SECONDS && !failed_lately
    }
}

/// What came of one try to issue or renew.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attempt {
    pub at: Timestamp,
    /// Until when the new certificate holds, or why there is none.
    pub outcome: Result<Timestamp, String>,
    pub output: Vec<String>,
}

/// Host ports zendiloOS or the NAS's file services use, also while they
/// are stopped (nginx would not start once they run); the web interface
/// takes none of them (80 and 443 are its own): SSH, rpcbind, SMB, NFS,
/// NFS's helpers on the ports the package fixes (mountd, statd, lockd),
/// WS-Discovery, LLMNR and mDNS.
pub const RESERVED_PORTS: [u16; 15] =
    [22, 111, 137, 138, 139, 445, 2049, 3702, 5353, 5355, 5357, 20048, 32765, 32766, 32767];

/// How the web interface is reached: plain HTTP on `http_port` only leads to
/// HTTPS (the session's cookie is HTTPS-only) and answers the authority's
/// HTTP challenges; HTTPS on `https_port` with the certificate made at
/// installation, or one of these.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WebServer {
    /// `None`: no plain HTTP at all.
    pub http_port: Option<u16>,
    pub https_port: u16,
    /// `None`: the self-signed certificate made at installation.
    pub certificate_id: Option<i64>,
}

impl Default for WebServer {
    fn default() -> Self {
        Self { http_port: Some(80), https_port: 443, certificate_id: None }
    }
}

impl WebServer {
    /// Checks the ports and that the certificate chosen is one that holds.
    pub fn check(&self, certificates: &[Certificate], now: Timestamp) -> Result<(), Invalid> {
        for port in self.http_port.into_iter().chain([self.https_port]) {
            if port == 0 {
                return Err(invalid("port 0 is no port"));
            }
            if RESERVED_PORTS.contains(&port) {
                return Err(invalid(format!("port {port} is used by another service of the NAS")));
            }
        }
        if self.http_port == Some(self.https_port) {
            return Err(invalid("HTTP and HTTPS need ports of their own"));
        }
        if let Some(id) = self.certificate_id {
            let certificate = certificates
                .iter()
                .find(|c| c.id == id)
                .ok_or_else(|| invalid("there is no such certificate"))?;
            if !certificate.is_valid(now) {
                return Err(invalid("the certificate is not issued yet or has expired"));
            }
        }
        Ok(())
    }

    /// The authority fetches HTTP challenges from port 80.
    pub fn answers_http_challenges(&self) -> bool {
        self.http_port == Some(80)
    }

    /// The ports it takes that `before` did not.
    pub fn new_ports(&self, before: &WebServer) -> Vec<u16> {
        let old: Vec<u16> = before.http_port.into_iter().chain([before.https_port]).collect();
        self.http_port.into_iter().chain([self.https_port]).filter(|port| !old.contains(port)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(seconds: i64) -> Timestamp {
        Timestamp::from_unix_seconds(seconds)
    }

    #[test]
    fn accounts_need_a_name_and_an_address() {
        let account = NewAccount {
            name: " Mine ".into(),
            server: AcmeServer::LetsEncrypt,
            email: " me@example.com ".into(),
        };
        let checked = account.clone().check().unwrap();
        assert_eq!((checked.name.as_str(), checked.email.as_str()), ("Mine", "me@example.com"));
        // The domain whatever its case, as acme.sh is given it.
        let upper = NewAccount { email: "Me.Myself@Example.COM".into(), ..account.clone() }.check().unwrap();
        assert_eq!(upper.email, "Me.Myself@example.com");
        for email in [
            "me",
            "me@localhost",
            "@example.com",
            "me@.example.com",
            "a b@example.com",
            "me@exa@mple.com",
            "-me@example.com",
            "me!@example.com",
            "me@exa_mple.com",
            "me@example.123",
        ] {
            let wrong = NewAccount { email: email.into(), ..account.clone() };
            assert!(wrong.check().is_err(), "{email}");
        }
        assert!(NewAccount { name: " ".into(), ..account }.check().is_err());
        for server in [AcmeServer::LetsEncrypt, AcmeServer::LetsEncryptStaging] {
            assert_eq!(AcmeServer::parse(server.as_str()), Some(server));
        }
        assert_eq!(AcmeServer::parse("zerossl"), None);
    }

    #[test]
    fn names_are_host_names_and_wildcards_lead() {
        for name in ["nas.example.com", "*.example.com", "a-b.c.example.org", "xn--bcher-kva.example"] {
            assert!(is_domain(name), "{name}");
        }
        for name in [
            "example",
            "-a.example.com",
            "a..example.com",
            "*example.com",
            "a.*.example.com",
            "NAS.example.com",
            "a_b.example.com",
            "192.168.1.1",
            "nas.example.com.",
        ] {
            assert!(!is_domain(name), "{name}");
        }
        assert!(!is_domain(&format!("{}.com", "a".repeat(64))));
        let checked =
            check_domains(&[" NAS.example.com ".into(), "".into(), "*.example.com".into()]).unwrap();
        assert_eq!(checked, ["nas.example.com", "*.example.com"]);
        assert!(check_domains(&[]).is_err());
        assert!(check_domains(&["a.example.com".into(), "A.example.com".into()]).is_err(), "twice");
        assert!(check_domains(&vec!["a.example.com".to_owned(); MAX_NAMES + 1]).is_err());
    }

    #[test]
    fn a_wildcard_needs_dns() {
        let new = NewCertificate {
            domains: vec!["*.example.com".into()],
            key: KeyType::default(),
            account_id: 1,
            challenge_id: 1,
            auto_renew: true,
        };
        assert!(new.clone().check(&ChallengeMethod::Http).is_err());
        let dns = ChallengeMethod::Dns { provider: "dns_desec".into(), values: BTreeMap::new() };
        assert!(new.check(&dns).is_ok());
        for key in [KeyType::Ec256, KeyType::Rsa2048, KeyType::Rsa4096] {
            assert_eq!(KeyType::parse(key.as_str()), Some(key));
        }
    }

    fn dns(provider: &str, values: &[(&str, &str)]) -> ChallengeMethod {
        ChallengeMethod::Dns {
            provider: provider.into(),
            values: values.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect(),
        }
    }

    #[test]
    fn dns_challenges_take_the_providers_values_and_keep_secrets_left_empty() {
        let ionos =
            dns("dns_ionos", &[("IONOS_PREFIX", " abc "), ("IONOS_SECRET", "s3cret")]).check(None).unwrap();
        assert_eq!(ionos, dns("dns_ionos", &[("IONOS_PREFIX", "abc"), ("IONOS_SECRET", "s3cret")]));
        // The secret is never shown again: empty means unchanged.
        let changed =
            dns("dns_ionos", &[("IONOS_PREFIX", "xyz"), ("IONOS_SECRET", "")]).check(Some(&ionos)).unwrap();
        assert_eq!(changed, dns("dns_ionos", &[("IONOS_PREFIX", "xyz"), ("IONOS_SECRET", "s3cret")]));
        // Another provider keeps nothing.
        assert!(dns("dns_desec", &[]).check(Some(&ionos)).is_err());
        assert!(dns("dns_ionos", &[("IONOS_PREFIX", "abc")]).check(None).is_err(), "the secret is missing");
        assert!(
            dns("dns_ionos", &[("IONOS_PREFIX", "a"), ("IONOS_SECRET", "b"), ("PATH", "/tmp")])
                .check(None)
                .is_err()
        );
        assert!(dns("dns_nowhere", &[]).check(None).is_err());
        assert!(dns("dns_ionos", &[("IONOS_PREFIX", "a\nb"), ("IONOS_SECRET", "b")]).check(None).is_err());
        let optional = dns("dns_cf", &[("CF_Token", "t"), ("CF_Zone_ID", "")]).check(None).unwrap();
        assert_eq!(optional, dns("dns_cf", &[("CF_Token", "t")]), "an empty optional value is left out");
        assert_eq!(ChallengeMethod::Http.check(Some(&ionos)), Ok(ChallengeMethod::Http));
        assert!(DNS_PROVIDERS.iter().all(|p| p.api.starts_with("dns_") && !p.variables.is_empty()));
    }

    fn certificate(not_after: Option<i64>) -> Certificate {
        Certificate {
            id: 1,
            domains: vec!["nas.example.com".into()],
            key: KeyType::Ec256,
            account_id: 1,
            challenge_id: 1,
            auto_renew: true,
            created_at: at(0),
            issued_at: not_after.map(|_| at(0)),
            not_after: not_after.map(at),
            last_attempt_at: None,
            last_error: None,
            last_output: Vec::new(),
            served: true,
        }
    }

    #[test]
    fn renewal_comes_a_month_before_the_end_and_waits_after_a_failure() {
        let day = 24 * 3600;
        let now = at(1_000 * day);
        assert!(!certificate(None).renewal_due(now), "never issued");
        assert!(!certificate(Some(1_031 * day)).renewal_due(now));
        assert!(certificate(Some(1_030 * day)).renewal_due(now));
        assert!(certificate(Some(999 * day)).renewal_due(now), "expired");
        let off = Certificate { auto_renew: false, ..certificate(Some(1_010 * day)) };
        assert!(!off.renewal_due(now));
        let failed = Certificate {
            last_attempt_at: Some(at(1_000 * day - 3600)),
            last_error: Some("dns".into()),
            ..certificate(Some(1_010 * day))
        };
        assert!(!failed.renewal_due(now), "tried an hour ago");
        assert!(failed.renewal_due(at(1_001 * day)), "a day later");
        assert!(certificate(Some(1_010 * day)).is_valid(now));
        assert!(!certificate(Some(999 * day)).is_valid(now));
        assert_eq!(certificate(None).common_name(), "nas.example.com");
    }

    #[test]
    fn the_web_server_takes_free_ports_and_a_certificate_that_holds() {
        let now = at(100);
        let issued = certificate(Some(200));
        assert!(WebServer::default().check(&[], now).is_ok());
        assert!(WebServer::default().answers_http_challenges());
        let moved = WebServer { http_port: Some(8080), https_port: 8443, certificate_id: Some(1) };
        assert!(moved.check(std::slice::from_ref(&issued), now).is_ok());
        assert!(!moved.answers_http_challenges());
        assert_eq!(moved.new_ports(&WebServer::default()), [8080, 8443]);
        assert_eq!(WebServer::default().new_ports(&WebServer::default()), Vec::<u16>::new());
        let https_only = WebServer { http_port: None, ..WebServer::default() };
        assert!(https_only.check(&[], now).is_ok());
        for wrong in [
            WebServer { http_port: Some(443), ..WebServer::default() },
            WebServer { https_port: 445, ..WebServer::default() },
            WebServer { https_port: 0, ..WebServer::default() },
            WebServer { certificate_id: Some(9), ..WebServer::default() },
        ] {
            assert!(wrong.check(std::slice::from_ref(&issued), now).is_err(), "{wrong:?}");
        }
        let expired = WebServer { certificate_id: Some(1), ..WebServer::default() };
        assert!(expired.check(&[certificate(Some(50))], now).is_err());
        assert!(expired.check(&[certificate(None)], now).is_err());
        // Ports of services that may not run yet: NFS's helpers, discovery.
        for port in [20048, 32765, 32766, 32767, 5355, 5357] {
            let https = WebServer { https_port: port, ..WebServer::default() };
            let http = WebServer { http_port: Some(port), ..WebServer::default() };
            assert!(https.check(&[], now).is_err() && http.check(&[], now).is_err(), "{port}");
        }
    }
}
