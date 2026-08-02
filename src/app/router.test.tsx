import { act, renderHook } from "@testing-library/react";
import type { ReactNode } from "react";
import { describe, expect, it } from "vitest";
import { AppRouter, matchPath, useAppRouter } from "@/app/router";

describe("matchPath", () => {
  it("extracts and decodes required parameters", () => {
    expect(matchPath("/people/:source/:sourceKey", "/people/pixiv/%E9%9D%92%E8%91%89")).toEqual({ source: "pixiv", sourceKey: "青葉" });
  });

  it("supports an optional trailing parameter", () => {
    expect(matchPath("/save/:source?", "/save")).toEqual({});
    expect(matchPath("/save/:source?", "/save/fanbox")).toEqual({ source: "fanbox" });
  });

  it("rejects partial and extra paths", () => {
    expect(matchPath("/works/:workId", "/works")).toBeNull();
    expect(matchPath("/works/:workId", "/works/1/edit")).toBeNull();
  });
});

describe("AppRouter", () => {
  it("navigates with a hash and keeps query parameters", () => {
    window.location.hash = "#/";
    const wrapper = ({ children }: { children: ReactNode }) => <AppRouter>{children}</AppRouter>;
    const { result } = renderHook(() => useAppRouter(), { wrapper });
    act(() => result.current.navigate("/library?q=%E5%89%B5%E4%BD%9C"));
    expect(window.location.hash).toBe("#/library?q=%E5%89%B5%E4%BD%9C");
  });
});
