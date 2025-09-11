# cdp-client

一个极简命令行工具，通过 Chrome DevTools Protocol (CDP) 连接 Node.js 调试端口，在 console 里输入 JavaScript 表达式并实时获取执行结果。

## 快速开始

1. 启动带调试端口的 Node 程序
   ```bash
   node --inspect your-script.js
   ```

   或者将运行中的进程发调试信号
   ```bash
   kill -USR1 <pid>
   ```


2. 运行本工具  
   ```bash
   cargo run --release -- console
   # 或直接使用编译好的二进制
   ./target/release/cdp-client console
   ```

3. 交互示例  
   ```
   cdp-client> 2 + 3
   5
   cdp-client> console.version
   'v20.19.0'
   cdp-client> exit
   Goodbye!
   ```
## 使用示例

1. 控制台模式 - 使用端口
cdp-client console -p 9229

2. 控制台模式 - 直接指定 WebSocket
cdp-client console -w "ws://127.0.0.1:9229/12345678-1234-1234-1234-123456789abc"

3. 控制台模式 - 使用 HTTP 端点
cdp-client console --http-url "http://localhost:9229/json"

4. 内存采样
cdp-client memory-sampling -p 9229 -d 30 -o memory_sampling.heapprofile

5. CPU profiling
cdp-client cpu-profile -p 9229 -d 10 -o cpu_profile.cpuprofile

6. 未来的堆快照 (已预留)
cdp-client heap-snapshot -p 9229 -o heap_snapshot.heapsnapshot

## 开发

```bash
cargo check        # 快速检查
cargo build --release
```

## 依赖

- tokio + tokio-tungstenite：异步 WebSocket
- serde / serde_json：CDP JSON 消息序列化
- anyhow：简洁错误处理

## 注意

- 仅支持本地调试（默认 127.0.0.1:9229）
- 输入 `exit` 或空行退出 REPL