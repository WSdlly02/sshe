# sshe

`sshe` 是一个 Linux 下的 SSH 智能包装器。它会读取逻辑主机配置，并发探测多个候选地址，优先选择延迟最低的地址发起真实 `ssh` 连接。

当前实现基于 Go，TOML 解析使用 `github.com/BurntSushi/toml`。

## 当前能力

- 支持一个逻辑主机对应多个候选地址
- 支持 `lowest_tcp_latency` 和 `lowest_icmp_latency`
- 支持全局默认值与主机级覆盖
- 支持并发探测多个候选地址
- 支持测速结果缓存，默认写入 `/run/user/<uid>/sshe/cache.toml`
- 支持 `--refresh-cache` 强制跳过缓存重测
- 支持将剩余参数原样透传给底层 `ssh`

## 默认配置路径

- `~/.ssh/sshe.toml`
- `~/.config/sshe.toml`
- `~/.config/sshe/config.toml`

## 构建

```bash
go build .
```

## 用法

```bash
./sshe my-pc
./sshe -c ./example/config.toml my-pc
./sshe --refresh-cache -c ./example/config.toml my-pc
./sshe -c ./example/config.toml -v my-pc -- hostname
```

`-v` 会输出所选地址、延迟、缓存来源和缓存路径。  
`--refresh-cache` 会跳过缓存并强制重新测速，然后用最新结果覆盖缓存。  
传给底层 `ssh` 的参数必须放在 `--` 之后。

## 缓存

- 默认缓存路径：`/run/user/<uid>/sshe/cache.toml`
- 默认缓存 TTL：`300` 秒
- 可通过 `global.cache_path` 和 `global.cache_ttl_sec` 覆盖
- 只有在“缓存未过期、端口一致、选址模式一致、缓存地址仍在候选集”时才会命中

## 配置示例

参考 [example/config.toml](/home/wsdlly02/Documents/sshe/example/config.toml)。

每个逻辑主机至少需要这些字段：

- `user`
- `port`
- `identity_file`
- `endpoints`

## 选择逻辑

- `lowest_tcp_latency`：并发对 `host:port` 建立 TCP 连接，按连接耗时选最优地址
- `lowest_icmp_latency`：并发调用 Linux `ping`，按 ICMP RTT 选最优地址

## 说明

- 当前版本只支持 Linux
- `lowest_icmp_latency` 依赖 Linux 风格的 `ping -c 1 -W <sec>`
