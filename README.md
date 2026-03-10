# sshe

`sshe` 是一个 SSH 智能包装器。它会读取逻辑主机配置，对候选地址做测速，然后选择延迟最低的地址发起真实 `ssh` 连接。

## 当前能力

- 支持从配置文件读取一个逻辑主机对应的多个候选地址
- 支持 `lowest_tcp_latency` 和 `lowest_icmp_latency` 两种优选模式
- 支持全局默认值与主机级覆盖
- 支持将额外参数原样透传给底层 `ssh`
- 支持 `~/.config/sshe/sshe.toml` 默认配置路径

## 用法

```bash
sshe my-pc
sshe -c ./example/config.toml my-pc
sshe -c ./example/config.toml -v my-pc -- hostname
```

`-v` 会输出所选地址和测得延迟。

## 配置示例

参考 [example/config.toml](/home/wsdlly02/Documents/sshe/example/config.toml)。

每个逻辑主机至少需要这些字段：

- `user`
- `port`
- `identity_file`
- `endpoints`

## 选择逻辑

- `lowest_tcp_latency`: 对 `host:port` 建立 TCP 连接，按连接耗时选最优地址
- `lowest_icmp_latency`: 调用系统 `ping`，按 ICMP RTT 选最优地址

## 说明

当前版本已经实现“测速 + 选址 + 执行 ssh”的主流程。
`cache_ttl_sec` 和 `cache_path` 仍保留在配置结构里，但尚未启用缓存逻辑。
