import { beforeEach, describe, expect, it } from "vitest";
import { defaultExportSettings, readExportSettings, toCompressOptions, writeExportSettings } from "./exportSettings";

describe("書き出しの決めごとを憶えておく", () => {
  beforeEach(() => window.localStorage.clear());

  /**
   * 画面を開くたびに初期値へ戻っていた。出力先フォルダーまで毎回選び直しで、
   * 数百冊を書き出す道具として手数が多すぎた。
   */
  it("次に開いたときも、前に選んだ出力先と設定のまま", () => {
    writeExportSettings({
      ...defaultExportSettings,
      outputDir: "D:/books",
      writingMode: "vertical",
      compression: { ...defaultExportSettings.compression, enabled: false, webpQuality: 55 },
    });

    const restored = readExportSettings();
    expect(restored.outputDir).toBe("D:/books");
    expect(restored.writingMode).toBe("vertical");
    expect(restored.compression.enabled).toBe(false);
    expect(restored.compression.webpQuality).toBe(55);
  });

  it("憶えが無ければ既定から始める", () => {
    expect(readExportSettings()).toEqual(defaultExportSettings);
  });

  /** 既定は横書き。縦書きは選んだときだけの上書きで、既定にはしない。 */
  it("何も選ばなければ、テンプレートの決め（＝横書き）のまま", () => {
    expect(defaultExportSettings.writingMode).toBeNull();
    expect(readExportSettings().writingMode).toBeNull();
  });

  it("壊れた憶えは既定へ落とす。書き出しが止まる理由にはしない", () => {
    window.localStorage.setItem("piep.epub-export.v1", "{ not json");
    expect(readExportSettings()).toEqual(defaultExportSettings);

    window.localStorage.setItem("piep.epub-export.v1", JSON.stringify({ writingMode: "斜め", compression: 5 }));
    const restored = readExportSettings();
    expect(restored.writingMode).toBeNull();
    expect(restored.compression).toEqual(defaultExportSettings.compression);
  });

  /** 空の入力欄をそのまま渡すと、端末側で数として読めない。 */
  it("入力途中の空欄は、指定なしとして渡す", () => {
    const options = toCompressOptions({ ...defaultExportSettings.compression, maxWidth: "", maxHeight: 1200 });
    expect(options.maxWidth).toBeNull();
    expect(options.maxHeight).toBe(1200);
  });
});
