---
layout: default
title: RemoText 使用说明
description: 通过 iroh 地址票据连接的轻量远程命令执行和文件传输工具
permalink: /
---

# RemoText 使用说明

RemoText 是一个无 GUI 的轻量远程命令执行工具。服务端启动后打印一个 `rt1_` 地址票据，客户端用这个地址和共享密码连接远端机器，不需要手动配置公网 IP、端口转发或 VPN。

当前版本支持：

- Windows、Linux、macOS。
- 远程执行命令，实时转发 stdout 和 stderr，并返回退出码。
- 上传和下载单个文件。
- 基于 iroh 的 QUIC 连接和地址票据。
- OPAQUE PAKE 密码认证，密码不会明文发送到网络上。
- 本地后台 session 管理器，适合脚本里反复执行一行命令。

## 安装和构建

从源码构建：

```bash
cargo build --release
```

构建完成后，二进制在：

```bash
target/release/remotext
```

开发时可以直接运行：

```bash
cargo run -- --help
```

如果已经把二进制复制到 `PATH` 中，后续示例可以直接使用 `remotext`。

## 启动服务端

在要被控制的机器上运行：

```bash
remotext server --password <password>
```

更推荐用环境变量传密码，避免把密码写进 shell 历史：

```bash
REMOTEXT_PASSWORD=<password> remotext server
```

服务端启动后会输出类似内容：

```text
RemoText server
network: iroh
protocol: remotext/1
name: remotext
address: rt1_...
data-dir: /path/to/RemoText
status: ready
```

把 `address: rt1_...` 后面的完整地址保存下来，客户端连接时会用到。

### 本地或 LAN 测试

如果只想做本机或局域网测试，可以关闭 relay 和 discovery：

```bash
remotext server --local-only --password <password>
```

客户端也需要对应加上 `--local-only`。

## 连接远端

先建立或预热后台 session：

```bash
remotext connect --addr rt1_... --password <password>
```

使用环境变量：

```bash
export REMOTEXT_ADDR=rt1_...
export REMOTEXT_PASSWORD=<password>
remotext connect
```

连接成功会输出：

```text
connected
```

默认后台 session 空闲 300 秒后退出。可以调整：

```bash
remotext connect --keepalive-secs 900
```

## 执行远程命令

最常用的一行命令形式：

```bash
REMOTEXT_ADDR=rt1_... REMOTEXT_PASSWORD=<password> remotext exec -- uname -a
```

`--` 很重要。它后面的内容会作为远程命令，不再被 RemoText 当成参数解析。

执行需要 shell 功能的命令时，显式调用远端 shell：

```bash
remotext exec -- sh -lc 'echo $HOME && id && pwd'
```

Windows `cmd.exe` 示例：

```powershell
$env:REMOTEXT_ADDR="rt1_..."
$env:REMOTEXT_PASSWORD="<password>"
remotext exec -- cmd /C dir
```

Windows PowerShell 示例：

```powershell
remotext exec -- powershell -NoProfile -Command "Get-ChildItem Env:"
```

### 绕过后台 session

默认 `exec` 会自动启动或复用本地后台 session。如果希望每次都新建直接连接：

```bash
remotext exec --no-session --addr rt1_... --password <password> -- uname -a
```

## 上传文件

把本地文件上传到远端路径：

```bash
remotext put --addr rt1_... --password <password> ./local.txt /tmp/remote.txt
```

使用环境变量后更适合脚本：

```bash
REMOTEXT_ADDR=rt1_... REMOTEXT_PASSWORD=<password> remotext put ./local.txt /tmp/remote.txt
```

上传行为：

- 本地文件按块流式传输。
- 服务端先写入临时文件。
- 完整接收后再重命名到目标路径。
- 传输失败时返回非零退出码。

## 下载文件

从远端下载文件到本地路径：

```bash
remotext get --addr rt1_... --password <password> /tmp/remote.txt ./local.txt
```

下载行为：

- 服务端按块流式发送文件。
- 客户端先写入临时文件。
- 完整接收后再重命名到目标路径。
- 传输失败时返回非零退出码。

## 认证和安全

RemoText 使用两层安全机制：

- iroh 提供加密 QUIC 连接和远端身份。
- RemoText 应用层使用 OPAQUE PAKE 做密码认证。

OPAQUE PAKE 的作用是让双方证明知道同一个密码，同时避免在网络上传输明文密码，也避免把可直接离线猜密码的 HMAC challenge-response 材料暴露给旁路观察者。

当前实现还会把 OPAQUE 派生出的 session key 用于绑定实际请求内容。客户端收到服务端握手身份后，也会确认它和 iroh 连接中的远端身份一致。

使用建议：

- 自动化脚本优先使用 `REMOTEXT_PASSWORD`，不要把密码写进命令历史。
- 不要在共享机器上通过命令行参数长期暴露密码。
- 服务端会以当前操作系统用户权限执行命令，不提供权限隔离。
- 如果以 root 或管理员启动服务端，认证客户端就拥有对应高权限。

## 后台 Session 行为

`connect`、默认 `exec`、默认 `put`、默认 `get` 会使用本地后台 session 管理器。

流程如下：

```text
remotext exec
  -> 查找本地 session 管理器
  -> 不存在则启动一个后台进程
  -> 通过 iroh 连接并认证远端服务端
  -> 提交命令或文件传输请求
  -> 当前终端实时接收输出
  -> 后台连接保持到 idle timeout
```

这样脚本可以像 `sshpass` 一样单行调用，同时多次调用不用反复建立完整连接。

## 常见问题

### 连接失败

检查：

- 地址是否完整复制了 `rt1_...`。
- 服务端是否仍在运行。
- 服务端和客户端是否都使用或都不使用 `--local-only`。
- 当前网络是否允许 iroh 使用的连接方式。

### 认证失败

检查：

- `--password` 或 `REMOTEXT_PASSWORD` 是否一致。
- 是否连接到了旧地址或另一台服务端。
- 服务端是否重建过数据目录和 iroh identity。

### 命令参数被 RemoText 解析了

在远程命令前加 `--`：

```bash
remotext exec --addr rt1_... --password <password> -- command --remote-flag
```

### 需要 shell 展开、管道或重定向

显式运行远端 shell：

```bash
remotext exec -- sh -lc 'ps aux | grep remotext'
```

## 退出码

常见退出码：

- `0`: RemoText 操作成功。
- `1`: 本地通用失败。
- `2`: CLI 参数错误。
- `10`: 连接失败。
- `11`: 认证失败。
- `12`: 协议版本不匹配。
- `20`: 远程命令启动失败。

远程命令自身返回非零退出码时，客户端会尽量返回同一个退出码。

## 进一步阅读

- [CLI 设计](./cli.md)
- [协议说明](./protocol.md)
- [安全设计](./security.md)
- [技术设计](./technical-design.md)
- [路线图](./roadmap.md)
