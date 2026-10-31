import { createTextDiff } from "./textDiff";

const scope = self as unknown as {
  onmessage: ((event: MessageEvent<{ before: string; after: string }>) => void) | null;
  postMessage(message: unknown): void;
};

scope.onmessage = (event) => scope.postMessage(createTextDiff(event.data.before, event.data.after));
