//! 存储层错误。面向用户的文案由 UI 层决定，这里只保留可判别的原因。

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("读写文件失败：{context}（{source}）")]
    Io {
        context: &'static str,
        #[source]
        source: std::io::Error,
    },

    #[error("配置文件格式不正确：{0}")]
    Decode(String),

    #[error("配置文件来自更新的程序版本（schema={0}），请先升级 SuperRemote")]
    UnsupportedSchema(u32),

    #[error("口令不正确")]
    WrongPassword,

    #[error("缺少口令：凭据已用主口令加密")]
    PasswordRequired,

    #[error("凭据文件与配置不一致：{0}")]
    Secret(String),

    /// `secrets.toml` 读得出来，但用当前数据密钥解不开。
    ///
    /// 这**不是**文件损坏：文件完好，只是 `config.toml` 里的 `wrapped_dek` 换了。
    /// 用户从别的机器恢复配置、或换了 Windows 登录账户后都会遇到。
    /// 这种情况必须报错，绝不能像损坏那样把文件改名备份再从空凭据启动。
    #[error("凭据文件与当前密钥不匹配：{0}")]
    KeyMismatch(String),

    #[error("系统密钥服务调用失败：{0}")]
    OsKey(String),

    #[error("目标数据目录里已有数据文件：{0}")]
    DataDirOccupied(String),

    /// 磁盘上的数据文件与本进程最后一次见到的不一致（另一个实例、编辑器或
    /// 备份恢复动过它）。保存被拒绝以避免盲写覆盖对方的改动。
    #[error("数据文件已被外部程序修改：{0}")]
    ExternalModified(String),
}

impl StoreError {
    pub fn io(context: &'static str, source: std::io::Error) -> Self {
        Self::Io { context, source }
    }

    /// 是否属于「文件读坏了，应当备份后重建」一类；口令错误与密钥不匹配都不算。
    pub fn is_corruption(&self) -> bool {
        matches!(self, Self::Decode(_) | Self::Secret(_))
    }
}
