use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::io::{self, Write};
use std::time::Duration;
use tokio::net::TcpStream;
use tokio_tungstenite::{connect_async, tungstenite::Message, MaybeTlsStream, WebSocketStream};
use url::Url;

#[derive(Parser)]
#[command(name = "cdp-client")]
#[command(about = "A Chrome DevTools Protocol client")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Interactive console mode
    Console {
        #[command(flatten)]
        connection: ConnectionArgs,
    },
    /// Capture memory allocation sampling
    MemorySampling {
        #[command(flatten)]
        connection: ConnectionArgs,
        /// Duration in seconds to capture sampling
        #[arg(short, long, default_value = "10")]
        duration: u64,
        /// Output file path
        #[arg(short, long, default_value = "memory_sampling.heapprofile")]
        output: String,
    },
    /// Capture CPU profile
    CpuProfile {
        #[command(flatten)]
        connection: ConnectionArgs,
        /// Duration in seconds to capture profile
        #[arg(short, long, default_value = "10")]
        duration: u64,
        /// Output file path
        #[arg(short, long, default_value = "cpu_profile.cpuprofile")]
        output: String,
    },
    /// Capture heap snapshot
    HeapSnapshot {
        #[command(flatten)]
        connection: ConnectionArgs,
        /// Output file path
        #[arg(short, long, default_value = "heap_snapshot.heapsnapshot")]
        output: String,
    },
}

#[derive(Parser)]
struct ConnectionArgs {
    /// Direct WebSocket URL
    #[arg(short, long, conflicts_with_all = ["http_url", "port"])]
    websocket: Option<String>,

    /// HTTP endpoint to get WebSocket URL from JSON list
    #[arg(long, conflicts_with = "port")]
    http_url: Option<String>,

    /// Port for localhost connection (will use http://localhost:PORT/json)
    #[arg(short, long)]
    port: Option<u16>,
}

#[derive(Serialize)]
struct CDPRequest {
    id: u64,
    method: String,
    params: serde_json::Value,
}

#[derive(Deserialize)]
struct CDPResponse {
    id: Option<u64>,
    result: Option<serde_json::Value>,
    error: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct CDPTarget {
    #[serde(rename = "webSocketDebuggerUrl")]
    websocket_debugger_url: String,
    #[serde(rename = "type")]
    target_type: String,
    #[allow(dead_code)]
    id: String,
    title: Option<String>,
}

struct CDPClient {
    ws: WebSocketStream<MaybeTlsStream<TcpStream>>,
    counter: u64,
}

impl CDPClient {
    async fn connect(args: &ConnectionArgs) -> Result<Self> {
        let ws_url = Self::resolve_websocket_url(args).await?;
        println!("Connecting to: {}", ws_url);

        let url = Url::parse(&ws_url).context("invalid CDP WebSocket URL")?;
        let (ws_stream, _) = connect_async(url).await?;

        Ok(CDPClient {
            ws: ws_stream,
            counter: 1,
        })
    }

    async fn resolve_websocket_url(args: &ConnectionArgs) -> Result<String> {
        if let Some(ws_url) = &args.websocket {
            return Ok(ws_url.clone());
        }

        let http_url = if let Some(url) = &args.http_url {
            url.clone()
        } else if let Some(port) = args.port {
            format!("http://localhost:{}/json", port)
        } else {
            // Default fallback
            "http://localhost:9229/json".to_string()
        };

        println!("Fetching target list from: {}", http_url);
        let response = reqwest::get(&http_url).await?;
        let targets: Vec<CDPTarget> = response.json().await?;

        if targets.is_empty() {
            anyhow::bail!("no targets found");
        }

        // For Node.js debugging, look for "node" type targets first
        // If not found, use the first available target
        let target = targets
            .iter()
            .find(|t| t.target_type == "node")
            .or_else(|| targets.iter().find(|t| t.target_type == "page"))
            .or_else(|| targets.first())
            .context("no suitable target found")?;

        println!(
            "Selected target: {} ({})",
            target.title.as_deref().unwrap_or("untitled"),
            target.target_type
        );

        Ok(target.websocket_debugger_url.clone())
    }

    async fn send_request(
        &mut self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value> {
        let payload = serde_json::to_string(&CDPRequest {
            id: self.counter,
            method: method.to_string(),
            params,
        })?;

        let req_id = self.counter;
        self.counter += 1;

        self.ws.send(Message::Text(payload)).await?;

        while let Some(msg) = self.ws.next().await {
            let msg = msg?;
            if let Message::Text(text) = msg {
                if let Ok(resp) = serde_json::from_str::<CDPResponse>(&text) {
                    if resp.id == Some(req_id) {
                        if let Some(err) = resp.error {
                            anyhow::bail!("CDP error: {}", err);
                        }
                        return Ok(resp.result.context("empty result")?);
                    }
                }
            }
        }
        anyhow::bail!("connection closed before response")
    }

    async fn evaluate(&mut self, expr: &str) -> Result<serde_json::Value> {
        self.send_request("Runtime.evaluate", json!({ "expression": expr }))
            .await
    }

    async fn start_console(&mut self) -> Result<()> {
        println!("CDP Console started. Type 'exit' to quit.");

        loop {
            print!("cdp-console> ");
            io::stdout().flush()?;
            let mut line = String::new();
            io::stdin().read_line(&mut line)?;
            let line = line.trim();

            if line.is_empty() || line == "exit" {
                break;
            }

            match self.evaluate(line).await {
                Ok(v) => {
                    if let Some(result_value) = v.get("result").and_then(|r| r.get("value")) {
                        println!("{}", result_value);
                    } else {
                        println!("{:?}", v);
                    }
                }
                Err(e) => eprintln!("Error: {}", e),
            }
        }

        println!("Goodbye!");
        Ok(())
    }

    async fn capture_memory_sampling(&mut self, duration: u64, output: &str) -> Result<()> {
        println!(
            "Starting memory allocation sampling for {} seconds...",
            duration
        );

        // Enable HeapProfiler domain
        self.send_request("HeapProfiler.enable", json!({})).await?;

        // Start sampling with default sample interval
        self.send_request(
            "HeapProfiler.startSampling",
            json!({
                "samplingInterval": 32768
            }),
        )
        .await?;

        // Wait for specified duration
        tokio::time::sleep(Duration::from_secs(duration)).await;

        // Stop sampling and get result
        let result = self
            .send_request("HeapProfiler.stopSampling", json!({}))
            .await?;

        // Extract the profile data and save in the original format
        if let Some(profile) = result.get("profile") {
            let profile_str = serde_json::to_string(profile)?;
            tokio::fs::write(output, profile_str).await?;
        } else {
            // Fallback: save the entire result
            let profile_str = serde_json::to_string(&result)?;
            tokio::fs::write(output, profile_str).await?;
        }

        println!("Memory sampling saved to: {}", output);
        Ok(())
    }

    async fn capture_cpu_profile(&mut self, duration: u64, output: &str) -> Result<()> {
        println!("Starting CPU profiling for {} seconds...", duration);

        // Enable Profiler domain
        self.send_request("Profiler.enable", json!({})).await?;

        // Start profiling
        self.send_request("Profiler.start", json!({})).await?;

        // Wait for specified duration
        tokio::time::sleep(Duration::from_secs(duration)).await;

        // Stop profiling and get result
        let result = self.send_request("Profiler.stop", json!({})).await?;

        // Extract the profile data and save in the original .cpuprofile format
        if let Some(profile) = result.get("profile") {
            let profile_str = serde_json::to_string(profile)?;
            tokio::fs::write(output, profile_str).await?;
        } else {
            // Fallback: save the entire result
            let profile_str = serde_json::to_string(&result)?;
            tokio::fs::write(output, profile_str).await?;
        }

        println!("CPU profile saved to: {}", output);
        Ok(())
    }

    async fn capture_heap_snapshot(&mut self, output: &str) -> Result<()> {
        println!("Taking heap snapshot...");

        // Enable HeapProfiler domain
        self.send_request("HeapProfiler.enable", json!({})).await?;

        // Force garbage collection first
        let _ = self
            .send_request("HeapProfiler.collectGarbage", json!({}))
            .await;

        let mut snapshot_chunks = Vec::new();

        self.ws
            .send(Message::Text(serde_json::to_string(&CDPRequest {
                id: self.counter,
                method: "HeapProfiler.takeHeapSnapshot".to_string(),
                params: json!({
                    "reportProgress": true
                }),
            })?))
            .await?;

        let req_id = self.counter;
        self.counter += 1;

        println!("Collecting snapshot data...");

        let mut finished = false;
        let mut request_completed = false;
        let start_time = std::time::Instant::now();
        let mut chunk_count = 0;

        while start_time.elapsed() < Duration::from_secs(120) {
            if let Some(msg) = self.ws.next().await {
                let msg = msg?;
                if let Message::Text(text) = msg {
                    if let Ok(event) = serde_json::from_str::<serde_json::Value>(&text) {
                        // 检查是否是我们的响应
                        if let Some(id) = event.get("id").and_then(|i| i.as_u64()) {
                            if id == req_id {
                                request_completed = true;
                                println!("DEBUG: takeHeapSnapshot request completed");
                                continue;
                            }
                        }

                        // 检查是否是事件
                        if let Some(method) = event.get("method").and_then(|m| m.as_str()) {
                            match method {
                                "HeapProfiler.addHeapSnapshotChunk" => {
                                    if let Some(chunk) = event
                                        .get("params")
                                        .and_then(|p| p.get("chunk"))
                                        .and_then(|c| c.as_str())
                                    {
                                        chunk_count += 1;
                                        snapshot_chunks.push(chunk.to_string());
                                        print!(".");
                                        io::stdout().flush().unwrap();

                                        // 输出每个块的大小信息
                                        if chunk_count <= 5 || chunk_count % 10 == 0 {
                                            println!(
                                                "\nDEBUG: Chunk {}: {} bytes",
                                                chunk_count,
                                                chunk.len()
                                            );
                                        }
                                    }
                                }
                                "HeapProfiler.reportHeapSnapshotProgress" => {
                                    if let Some(params) = event.get("params") {
                                        if let Some(is_finished) =
                                            params.get("finished").and_then(|f| f.as_bool())
                                        {
                                            if is_finished {
                                                finished = true;
                                                println!("\nSnapshot collection finished. Total chunks: {}", chunk_count);

                                                // 等待更长时间确保所有数据块都收到
                                                if snapshot_chunks.is_empty() {
                                                    println!("DEBUG: Finished but no chunks yet, waiting 2 seconds...");
                                                    tokio::time::sleep(Duration::from_secs(2))
                                                        .await;
                                                } else {
                                                    // 即使有数据块，也等待一下确保没有遗漏
                                                    println!("DEBUG: Waiting 1 second for any remaining chunks...");
                                                    tokio::time::sleep(Duration::from_secs(1))
                                                        .await;
                                                }
                                            }
                                        }

                                        // 显示进度
                                        if let (Some(done), Some(total)) = (
                                            params.get("done").and_then(|d| d.as_u64()),
                                            params.get("total").and_then(|t| t.as_u64()),
                                        ) {
                                            if total > 0 {
                                                let progress = (done * 100) / total;
                                                print!("\rProgress: {}%", progress);
                                                io::stdout().flush().unwrap();
                                            }
                                        }
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                }
            } else {
                println!("DEBUG: WebSocket connection closed");
                break;
            }

            // 如果已经完成，再等待一段时间收集剩余的数据块
            if finished {
                // 继续等待一小段时间，看是否还有数据块
                let mut additional_wait = 0;
                while additional_wait < 5 {
                    // 最多再等5秒
                    tokio::time::sleep(Duration::from_millis(200)).await;
                    additional_wait += 1;

                    // 检查是否还有消息
                    if let Ok(Some(msg)) =
                        tokio::time::timeout(Duration::from_millis(100), self.ws.next()).await
                    {
                        if let Ok(msg) = msg {
                            if let Message::Text(text) = msg {
                                if let Ok(event) = serde_json::from_str::<serde_json::Value>(&text)
                                {
                                    if let Some(method) =
                                        event.get("method").and_then(|m| m.as_str())
                                    {
                                        if method == "HeapProfiler.addHeapSnapshotChunk" {
                                            if let Some(chunk) = event
                                                .get("params")
                                                .and_then(|p| p.get("chunk"))
                                                .and_then(|c| c.as_str())
                                            {
                                                chunk_count += 1;
                                                snapshot_chunks.push(chunk.to_string());
                                                println!(
                                                    "DEBUG: Late chunk {}: {} bytes",
                                                    chunk_count,
                                                    chunk.len()
                                                );
                                                additional_wait = 0; // 重置等待计数器
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                break;
            }
        }

        if start_time.elapsed() >= Duration::from_secs(120) {
            anyhow::bail!("Heap snapshot collection timed out after 2 minutes");
        }

        if snapshot_chunks.is_empty() {
            anyhow::bail!(
                "No heap snapshot data received. Request completed: {}, Finished: {}",
                request_completed,
                finished
            );
        }

        println!("DEBUG: Combining {} chunks...", snapshot_chunks.len());
        let full_snapshot = snapshot_chunks.join("");

        // 验证 JSON 格式
        println!("DEBUG: Validating JSON format...");
        match serde_json::from_str::<serde_json::Value>(&full_snapshot) {
            Ok(_) => println!("DEBUG: JSON format is valid"),
            Err(e) => {
                println!("DEBUG: JSON validation failed: {}", e);
                println!(
                    "DEBUG: First 500 chars: {}",
                    &full_snapshot[..std::cmp::min(500, full_snapshot.len())]
                );
                println!(
                    "DEBUG: Last 500 chars: {}",
                    &full_snapshot[std::cmp::max(0, full_snapshot.len().saturating_sub(500))..]
                );
            }
        }

        tokio::fs::write(output, &full_snapshot).await?;

        println!(
            "\nHeap snapshot saved to: {} ({} bytes, {} chunks)",
            output,
            full_snapshot.len(),
            snapshot_chunks.len()
        );
        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Console { connection } => {
            let mut client = CDPClient::connect(&connection).await?;
            client.start_console().await?;
        }
        Commands::MemorySampling {
            connection,
            duration,
            output,
        } => {
            let mut client = CDPClient::connect(&connection).await?;
            client.capture_memory_sampling(duration, &output).await?;
        }
        Commands::CpuProfile {
            connection,
            duration,
            output,
        } => {
            let mut client = CDPClient::connect(&connection).await?;
            client.capture_cpu_profile(duration, &output).await?;
        }
        Commands::HeapSnapshot { connection, output } => {
            let mut client = CDPClient::connect(&connection).await?;
            client.capture_heap_snapshot(&output).await?;
        }
    }

    Ok(())
}
