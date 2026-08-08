use std::net::SocketAddr;
use std::path::PathBuf;

use ipnet::IpNet;
use url::Url;

use crate::shared::error::{AppError, ErrorCode};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 选择开发环境默认值或仅生产环境适用的启动安全要求。
pub enum RunMode {
    /// 允许 HTTP 源、空的受信任代理列表及缺失的 Web 构建产物。
    Development,
    /// 需要 HTTPS、显式配置的受信任代理以及已构建的 Web 产物才会就绪。
    Production,
}

#[derive(Clone, Debug)]
/// Core HTTP、持久化、发现和静态 Web 适配器共享的启动输入。
///
/// [`Self::load`] 返回的 `public_origin` 始终只含 HTTP(S) 授权信息，代理网络始终可解析。
/// 跨字段及文件系统要求会由 [`Self::readiness_issues`] 报告，确保业务路由关闭时
/// 存活探针仍可用。
pub struct AppConfig {
    /// 控制适用哪些就绪约束。
    pub mode: RunMode,
    /// HTTP 服务监听的套接字地址。
    pub listen: SocketAddr,
    /// 包含数据库和备份目录的持久化配置卷。
    pub config_dir: PathBuf,
    /// 用于同源请求校验的规范外部源。
    pub public_origin: Url,
    /// 允许提供最左侧转发客户端地址的代理源网络。
    pub trusted_proxy_cidrs: Vec<IpNet>,
    /// 声明受能力约束媒体根目录的 JSON 文件。
    pub deployment_roots_file: PathBuf,
    /// 提供不可变 Web 资源与 SPA 入口的目录。
    pub web_dist: PathBuf,
}

impl AppConfig {
    /// 从 `MEDIAFLOW_*` 环境变量加载 Core 配置。
    ///
    /// # Errors
    ///
    /// 当环境变量值格式错误或不符合源的形状要求时返回 [`AppError`]。
    pub fn load() -> Result<Self, AppError> {
        let mode = match env_value("MEDIAFLOW_MODE").as_deref() {
            None | Some("development") => RunMode::Development,
            Some("production") => RunMode::Production,
            Some(_) => {
                return Err(config_error(
                    "MEDIAFLOW_MODE must be development or production",
                ));
            }
        };
        let listen = env_value("MEDIAFLOW_LISTEN")
            .unwrap_or_else(|| "127.0.0.1:3000".to_owned())
            .parse()
            .map_err(|_| config_error("MEDIAFLOW_LISTEN must be a socket address"))?;
        let config_dir = PathBuf::from(
            env_value("MEDIAFLOW_CONFIG_DIR").unwrap_or_else(|| "/config".to_owned()),
        );
        let public_origin = Url::parse(
            &env_value("MEDIAFLOW_PUBLIC_ORIGIN").unwrap_or_else(|| format!("http://{listen}")),
        )
        .map_err(|_| config_error("MEDIAFLOW_PUBLIC_ORIGIN must be an absolute URL"))?;
        if public_origin.cannot_be_a_base()
            || !matches!(public_origin.scheme(), "http" | "https")
            || public_origin.host_str().is_none()
            || !public_origin.username().is_empty()
            || public_origin.password().is_some()
            || public_origin.query().is_some()
            || public_origin.fragment().is_some()
            || public_origin.path() != "/"
        {
            return Err(config_error(
                "MEDIAFLOW_PUBLIC_ORIGIN must contain only scheme and authority",
            ));
        }
        let trusted_proxy_cidrs = env_value("MEDIAFLOW_TRUSTED_PROXY_CIDRS")
            .map(|value| {
                value
                    .split(',')
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(|value| {
                        value.parse().map_err(|_| {
                            config_error("MEDIAFLOW_TRUSTED_PROXY_CIDRS contains an invalid CIDR")
                        })
                    })
                    .collect()
            })
            .transpose()?
            .unwrap_or_default();
        let deployment_roots_file = env_value("MEDIAFLOW_DEPLOYMENT_ROOTS_FILE")
            .map_or_else(|| config_dir.join("deployment-roots.json"), PathBuf::from);
        let web_dist =
            PathBuf::from(env_value("MEDIAFLOW_WEB_DIST").unwrap_or_else(|| "/app/web".to_owned()));

        Ok(Self {
            mode,
            listen,
            config_dir,
            public_origin,
            trusted_proxy_cidrs,
            deployment_roots_file,
            web_dist,
        })
    }

    #[must_use]
    /// 报告业务路由开启前必须修正的全部启动条件。
    ///
    /// 此方法读取部署根目录文件，并在生产环境检查 `index.html`；它不会修改配置或创建缺失文件。
    /// 空结果仅表示配置通过这些静态检查，并不表示数据库可访问。
    pub fn readiness_issues(&self) -> Vec<&'static str> {
        let mut issues = Vec::new();
        if !matches!(self.public_origin.scheme(), "http" | "https") {
            issues.push("public origin must use HTTP or HTTPS");
        }
        if self.mode == RunMode::Production && self.public_origin.scheme() != "https" {
            issues.push("production public origin must use HTTPS");
        }
        if self.mode == RunMode::Production && self.trusted_proxy_cidrs.is_empty() {
            issues.push("production trusted proxy CIDRs must be configured");
        }
        if self
            .trusted_proxy_cidrs
            .iter()
            .any(|network| network.prefix_len() == 0)
        {
            issues.push("trusted proxy CIDRs must not trust the entire address space");
        }
        if !self.deployment_roots_file.is_file() {
            issues.push("deployment roots file is missing");
        } else if crate::discovery::capability::DeploymentRootSet::load(
            &self.deployment_roots_file,
            self.mode,
        )
        .is_err()
        {
            issues.push("deployment roots configuration is invalid");
        }
        if self.mode == RunMode::Production && !self.web_dist.join("index.html").is_file() {
            issues.push("production web distribution is missing");
        }
        issues
    }
}

fn env_value(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

fn config_error(message: &'static str) -> AppError {
    AppError::new(ErrorCode::ConfigInvalid, message)
}
