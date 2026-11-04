import { afterEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { rememberWindowColorScheme, showStartupFailure, waitForLibrary } from "@/services/startupApi";
import indexHtml from "../../index.html?raw";
import screenColours from "../../public/startup.css?raw";
import windowColours from "../../src-tauri/src/commands/startup.rs?raw";
import appColours from "../styles/app.css?raw";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(), convertFileSrc: (path: string) => path }));

function enterTauri() {
  Object.defineProperty(window, "__TAURI_INTERNALS__", { value: {}, configurable: true });
}

afterEach(() => {
  Reflect.deleteProperty(window, "__TAURI_INTERNALS__");
  vi.mocked(invoke).mockReset();
  document.body.innerHTML = "";
});

describe("startup gate", () => {
  it("waits for the library inside the app", async () => {
    enterTauri();
    vi.mocked(invoke).mockResolvedValue(undefined);
    await waitForLibrary();
    expect(invoke).toHaveBeenCalledWith("startup_wait_for_library");
  });

  it("does not wait in a plain browser, which has no library to open", async () => {
    await waitForLibrary();
    await rememberWindowColorScheme("dark");
    expect(invoke).not.toHaveBeenCalled();
  });

  it("passes on why the library could not open", async () => {
    enterTauri();
    vi.mocked(invoke).mockRejectedValue("Failed to open database");
    await expect(waitForLibrary()).rejects.toBe("Failed to open database");
  });
});

describe("showStartupFailure", () => {
  it("keeps the startup screen and says what went wrong", () => {
    document.body.innerHTML = `<div class="startup-splash"><p class="startup-splash__message">準備中</p></div>`;
    showStartupFailure("<b>Failed</b> to open database");
    const splash = document.querySelector(".startup-splash");
    expect(splash).toHaveAttribute("data-failed");
    expect(splash).toHaveTextContent("piepを起動できませんでした");
    // The detail comes from the backend and may hold anything, so it stays text.
    expect(splash?.querySelector(".startup-splash__detail")?.textContent).toBe("<b>Failed</b> to open database");
    expect(splash?.querySelector("b")).toBeNull();
  });
});

describe("startup colours", () => {
  // The window, the startup screen and the app are three separate definitions. A
  // mismatch shows as a flash at each hand-over.
  it.each([
    { name: "light", hex: "#f5f7fa", rust: "LIGHT_BACKGROUND: Color = Color(0xF5, 0xF7, 0xFA, 0xFF)" },
    { name: "dark", hex: "#0d1117", rust: "DARK_BACKGROUND: Color = Color(0x0D, 0x11, 0x17, 0xFF)" },
  ])("agree in $name", ({ hex, rust }) => {
    expect(windowColours).toContain(rust);
    expect(screenColours).toContain(`--startup-background: ${hex}`);
    expect(appColours).toContain(hex);
  });

  it("keeps index.html free of <style>, which would switch off Mantine's runtime styles", () => {
    expect(indexHtml).not.toMatch(/<style[\s>]/);
  });
});
