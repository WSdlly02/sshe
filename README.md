# sshe

这个仓库当前包含三个二进制目标：

- `ssher`: 供 OpenSSH `ProxyCommand` 调用的地址选择器与 TCP 代理
- `sshea`: 面向 AI agent 的 SSH 客户端，后续实现
- `sshed`: 面向 AI agent 的 SSH 服务端，后续实现

## ssher

`ssher` 的职责很单一：

- 根据逻辑主机名选择当前最优 endpoint
- 建立到目标 `host:port` 的 TCP 连接
- 将本地 `stdin/stdout` 直接桥接到该 TCP 连接

`ssher` 不负责：

- 执行 `ssh`
- 处理 `user`
- 处理 `identity_file`
- 透传远端命令

这些都交给 OpenSSH 自己处理。

## ssh_config 接入

```sshconfig
Host my-pc
  User wsdlly02
  IdentityFile ~/.ssh/id_ed25519
  ProxyCommand /path/to/ssher --host %n --port %p
```

这样 `ssh`、`scp`、`rsync` 等工具都会通过 `ssher` 先选择地址，再继续使用现有 OpenSSH 工具链。
不要在 `ssh_config` 中为这类条目定义 `HostName`，让逻辑主机名直接作为 `ssher` 的配置键。

## 配置文件

默认会按顺序查找以下配置路径：

- `~/.ssh/ssher.toml`
- `~/.config/ssher.toml`
- `~/.config/sshe/ssher_config.toml`

示例配置见 [example/ssher.toml](/home/wsdlly02/Documents/sshe/example/ssher.toml)。

配置文件现在只保留“探测和选址”相关字段：

- `probe_timeout_ms`
- `cache_ttl_sec`
- `cache_path`
- `selection_mode`
- `endpoints`

## 使用方式

直接调试时可以这样运行：

```bash
cargo run --bin ssher -- --host my-pc --port 22
cargo run --bin ssher -- --host my-pc --port 22 --refresh-cache -v
```

`-v` 只会输出到 `stderr`，不会污染 `ProxyCommand` 的 `stdout` 数据流。  
`--refresh-cache` 会跳过缓存并强制重新测速，然后用最新结果覆盖缓存。

## 缓存

- 默认缓存路径：`/run/user/<uid>/sshe/ssher_cache.toml`
- 默认缓存 TTL：`300` 秒
- 可通过 `global.cache_path` 和 `global.cache_ttl_sec` 覆盖
- 只有在“缓存未过期、端口一致、选址模式一致、缓存地址仍在候选集”时才会命中

## 选择逻辑

- `lowest_tcp_latency`: 对所有 `endpoint:port` 全量并发建立 TCP 连接，首个成功完成的地址优先
- `lowest_icmp_latency`: 对所有 `endpoint` 全量并发调用 Linux `ping`，首个成功完成的地址优先

## 说明

- 当前版本只支持 Linux
- `lowest_icmp_latency` 依赖 Linux 风格的 `ping -c 1 -W <sec>`
- `sshea` 和 `sshed` 目前仍是占位入口
