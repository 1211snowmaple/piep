// 検証用 HTTPS 接続を終端せず転送し、PC から Android への応答を遅らせる。
import net from "node:net";

const host = process.argv[2] ?? "127.0.0.1";
const sourcePort = Number(process.argv[3] ?? "38777");
const proxyPort = Number(process.argv[4] ?? "38778");
const delayMs = Number(process.argv[5] ?? "300");
if (![sourcePort, proxyPort, delayMs].every(Number.isFinite) || delayMs < 0 || sourcePort === proxyPort) {
  throw new Error("Usage: node slow-tcp-proxy.mjs <private-ip> <source-port> <proxy-port> <delay-ms>");
}

const server = net.createServer((client) => {
  const upstream = net.connect({ host, port: sourcePort });
  client.pipe(upstream);
  upstream.on("data", (chunk) => {
    upstream.pause();
    setTimeout(() => {
      if (!client.destroyed) client.write(chunk);
      upstream.resume();
    }, delayMs);
  });
  client.on("error", () => upstream.destroy());
  upstream.on("error", () => client.destroy());
  client.on("close", () => upstream.destroy());
  upstream.on("close", () => client.destroy());
});

server.listen(proxyPort, host, () => {
  console.log(`Slow TCP proxy listening on ${host}:${proxyPort}; forwarding to ${sourcePort} with ${delayMs} ms per response chunk`);
});
