pub mod cache;
pub mod cli;
pub mod core;
pub mod dns;
pub mod git;
pub mod intercepts;
pub mod mitm;
pub mod proxy;
pub mod rules;
pub mod system;
pub mod utils;

pub use core::hub::HakimiHub;

/// 测试辅助模块
pub mod test_utils {
    use std::sync::Once;

    static INIT: Once = Once::new();

    /// 确保 TLS 加密库只初始化一次
    pub fn ensure_crypto_provider() {
        INIT.call_once(|| {
            let _ = rustls::crypto::ring::default_provider().install_default();
        });
    }
}