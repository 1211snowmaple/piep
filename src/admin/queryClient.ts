import { QueryCache, QueryClient } from "@tanstack/react-query";
import { ApiFailure } from "./api";

type Listener = () => void;
const expiredListeners = new Set<Listener>();

/** 要求がログイン切れで断られたら呼ぶ。画面は消さず、上にログインを出す。 */
export function onSessionExpired(listener: Listener): () => void {
  expiredListeners.add(listener);
  return () => expiredListeners.delete(listener);
}

export function notifyIfExpired(error: unknown): void {
  if (error instanceof ApiFailure && error.status === 401 && error.code === "unauthorized") {
    for (const listener of expiredListeners) listener();
  }
}

export const adminQueryClient = new QueryClient({
  queryCache: new QueryCache({ onError: notifyIfExpired }),
  defaultOptions: {
    queries: {
      retry: (failures, error) => !(error instanceof ApiFailure) && failures < 1,
      refetchOnWindowFocus: true,
      // 読めなくなっても、最後に読んだ内容を残す。
      staleTime: 5_000,
    },
  },
});
