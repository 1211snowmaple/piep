// Pair the two local emulator WebViews to a running desktop dev build without
// printing or storing the short-lived pairing payload.
async function evaluate(port, urlPrefix, expression) {
  const tabs = await fetch(`http://127.0.0.1:${port}/json`).then((response) => response.json());
  const tab = tabs.find((item) => item.type === "page" && item.url.startsWith(urlPrefix));
  if (!tab) throw new Error(`piep WebView missing on CDP port ${port}`);
  const socket = new WebSocket(tab.webSocketDebuggerUrl);
  return await new Promise((resolve, reject) => {
    const timeout = setTimeout(() => { socket.close(); reject(new Error(`CDP timeout on ${port}`)); }, 15_000);
    socket.addEventListener("open", () => socket.send(JSON.stringify({
      id: 1, method: "Runtime.evaluate", params: { expression, awaitPromise: true, returnByValue: true },
    })));
    socket.addEventListener("message", (event) => {
      const message = JSON.parse(event.data);
      if (message.id !== 1) return;
      clearTimeout(timeout);
      socket.close();
      if (message.error || message.result?.exceptionDetails) reject(new Error(`CDP evaluation failed on ${port}`));
      else resolve(message.result?.result?.value);
    });
    socket.addEventListener("error", () => { clearTimeout(timeout); reject(new Error(`CDP connection failed on ${port}`)); });
  });
}

const viaUi = process.argv.includes("--via-ui");
for (const [port, deviceName] of [[9222, "piep phone emulator"], [9223, "piep tablet emulator"]]) {
  const pairing = await evaluate(9230, "http://localhost:1420/", 'window.__TAURI_INTERNALS__.invoke("mobile_bridge_pairing")');
  let status;
  if (viaUi) {
    const payload = JSON.stringify(pairing.payload);
    const name = JSON.stringify(deviceName);
    status = await evaluate(port, "http://tauri.localhost", `(async () => {
      document.querySelectorAll(".bottom-item")[2].click();
      await new Promise(resolve => setTimeout(resolve, 100));
      const details = document.querySelector(".connection-repair");
      if (details) details.open = true;
      const textarea = document.querySelector(".connection-registration textarea");
      const nameInput = document.querySelector(".connection-registration input");
      if (!textarea || !nameInput) return { error: "settings registration form missing" };
      Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value").set.call(nameInput, ${name});
      nameInput.dispatchEvent(new Event("input", { bubbles: true }));
      Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value").set.call(textarea, ${payload});
      textarea.dispatchEvent(new Event("input", { bubbles: true }));
      await new Promise(resolve => setTimeout(resolve, 100));
      const button = document.querySelector(".connection-registration .primary-button");
      if (!button || button.disabled) return { error: "settings registration button disabled" };
      button.click();
      for (let attempt = 0; attempt < 100; attempt++) {
        await new Promise(resolve => setTimeout(resolve, 100));
        const result = await window.__TAURI_INTERNALS__.invoke("pc_connection_status");
        if (result.paired && result.reachable) return { paired: true, reachable: true, badge: document.querySelector(".connection-settings .connection-badge")?.textContent };
      }
      return { error: document.querySelector(".connection-settings .inline-alert")?.textContent ?? "settings registration timed out" };
    })()`);
  } else {
    const args = JSON.stringify({ payload: pairing.payload, deviceName });
    status = await evaluate(port, "http://tauri.localhost", `window.__TAURI_INTERNALS__.invoke("pc_pair", ${args}).then(s => ({ paired: s.paired, reachable: s.reachable }))`);
  }
  if (!status?.paired || !status?.reachable) throw new Error(`${deviceName} did not connect`);
  console.log(`${deviceName}: paired and connected${viaUi ? " via settings" : ""}`);
}
