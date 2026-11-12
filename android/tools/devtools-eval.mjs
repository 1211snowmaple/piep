// Android WebView の開発用 DevTools へ一度だけ式を送り、結果を表示する。
const expression = process.argv[2] ?? await new Promise((resolve) => {
  let input = "";
  process.stdin.setEncoding("utf8");
  process.stdin.on("data", (chunk) => { input += chunk; });
  process.stdin.on("end", () => resolve(input));
});
if (!expression) throw new Error("JavaScript expression required");
const port = Number(process.env.PIEP_DEVTOOLS_PORT ?? "9222");
const urlPrefix = process.env.PIEP_DEVTOOLS_URL_PREFIX ?? "http://tauri.localhost";
const tabs = await fetch(`http://127.0.0.1:${port}/json`).then((response) => response.json());
const tab = tabs.find((item) => item.type === "page" && item.url.startsWith(urlPrefix));
if (!tab) throw new Error("piep WebView not found");
const socket = new WebSocket(tab.webSocketDebuggerUrl);
const result = await new Promise((resolve, reject) => {
  const timer = setTimeout(() => { socket.close(); reject(new Error("DevTools timed out")); }, 10000);
  socket.addEventListener("open", () => socket.send(JSON.stringify({
    id: 1,
    method: "Runtime.evaluate",
    params: { expression, awaitPromise: true, returnByValue: true },
  })));
  socket.addEventListener("message", (event) => {
    const message = JSON.parse(event.data);
    if (message.id !== 1) return;
    clearTimeout(timer);
    socket.close();
    if (message.error || message.result?.exceptionDetails) reject(new Error(JSON.stringify(message.error ?? message.result.exceptionDetails)));
    else resolve(message.result?.result?.value);
  });
  socket.addEventListener("error", () => { clearTimeout(timer); reject(new Error("DevTools connection failed")); });
});
console.log(JSON.stringify(result));
