pub const ALPN: &[u8] = b"sshe/1";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const MAX_FRAME: usize = 4 * 1024 * 1024;
pub const OUTPUT_LIMIT: usize = 256 * 1024;
pub const MAX_EXEC_SECONDS: u64 = 60;
/// Upper bound on records returned by one history query; keeps replies under MAX_FRAME.
pub const MAX_HISTORY_QUERY: usize = 1000;
