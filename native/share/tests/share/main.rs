//! `fluxdown_share` 的集成测试（单一测试二进制，按模块分组）。全部离线，只使用回环地址。
//!
//! 模块带 `#[cfg(test)]`：该二进制只在 `cargo test` 下编译，这样 clippy 的
//! `allow-unwrap-in-tests` 对辅助函数同样生效。

#[cfg(test)]
mod common;
#[cfg(test)]
mod e2e;
#[cfg(test)]
mod endpoint;
#[cfg(test)]
mod hashing;
#[cfg(test)]
mod http;
#[cfg(test)]
mod sources;
#[cfg(test)]
mod swarm;
#[cfg(test)]
mod ticket;
