#!/usr/bin/env node

const CDP = require('chrome-remote-interface');
const fs = require('fs');

// 解析命令行参数
const args = process.argv.slice(2);
const port = args.find(arg => arg.startsWith('--port='))?.split('=')[1] || 9229;
const duration = args.find(arg => arg.startsWith('--duration='))?.split('=')[1] || 10000;
const interval = args.find(arg => arg.startsWith('--interval='))?.split('=')[1] || 100;
const output = args.find(arg => arg.startsWith('--output='))?.split('=')[1];

async function captureProfile() {
  let client;
  try {
    client = await CDP({ port: parseInt(port) });
    const { Profiler } = client;
    
    await Profiler.enable();
    console.log('Profiler 已启用');
    
    await Profiler.start({ samplingInterval: parseInt(interval) });
    console.log(`开始 CPU profiling，持续 ${duration/1000} 秒...`);
    
    await new Promise(resolve => setTimeout(resolve, parseInt(duration)));
    
    const { profile } = await Profiler.stop();
    console.log('CPU profiling 完成！');
    
    // 使用自定义文件名或默认文件名
    const filename = output || `cpu-profile-${Date.now()}.cpuprofile`;
    fs.writeFileSync(filename, JSON.stringify(profile, null, 2));
    console.log(`Profile 已保存到 ${filename}`);
    console.log('可以直接拖拽到 Chrome DevTools Performance 面板查看');
    
  } catch (error) {
    console.error('错误:', error);
    process.exit(1);
  } finally {
    if (client) {
      await client.close();
    }
  }
}

// 显示帮助信息
if (args.includes('--help') || args.includes('-h')) {
  console.log(`
用法: node cpu-profiler.js [选项]

选项:
  --port=<端口>        Chrome DevTools 端口 (默认: 9229)
  --duration=<毫秒>    Profiling 持续时间 (默认: 10000)
  --interval=<毫秒>    采样间隔 (默认: 100)
  --output=<文件名>    输出文件名 (默认: cpu-profile-<时间戳>.cpuprofile)
  --help, -h          显示帮助信息

示例:
  node cpu-profiler.js
  node cpu-profiler.js --port=9230 --duration=5000
  node cpu-profiler.js --output=my-profile.cpuprofile
  `);
  process.exit(0);
}

captureProfile();
