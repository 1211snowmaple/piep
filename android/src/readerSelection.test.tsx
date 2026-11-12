import { act, cleanup, renderHook } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import type { PointerEvent } from "react";
import { useReaderSelection } from "./readerSelection";

afterEach(() => { cleanup(); vi.useRealTimers(); });
it("短いタップと動いた指は選択にせず、止めて長押しした指だけを選択へ渡す", () => {
  vi.useFakeTimers();
  const root = document.createElement("main");
  const event = (x = 0) => ({ currentTarget: root, target: root, pointerId: 1, pointerType: "touch", isPrimary: true, clientX: x, clientY: 0 }) as unknown as PointerEvent<HTMLElement>;
  const { result } = renderHook(useReaderSelection);
  act(() => result.current.handlers.onPointerDown(event()));
  act(() => vi.advanceTimersByTime(100));
  act(() => result.current.handlers.onPointerUp());
  act(() => vi.advanceTimersByTime(500));
  expect(result.current.enabled).toBe(false);
  act(() => result.current.handlers.onPointerDown(event()));
  act(() => result.current.handlers.onPointerMove(event(15)));
  act(() => vi.advanceTimersByTime(500));
  expect(result.current.enabled).toBe(false);
  act(() => result.current.handlers.onPointerDown(event()));
  act(() => vi.advanceTimersByTime(280));
  expect(result.current.enabled).toBe(true);
  expect(result.current.selecting()).toBe(true);
});
