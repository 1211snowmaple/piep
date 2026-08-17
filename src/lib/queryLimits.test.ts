import { InfiniteQueryObserver, QueryClient } from "@tanstack/react-query";
import { describe, expect, it } from "vitest";
import { boundedInfiniteListOptions, INFINITE_LIST_MAX_PAGES } from "@/lib/queryLimits";

describe("bounded infinite query options", () => {
  it("evicts old pages after the shared cache limit", async () => {
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const observer = new InfiniteQueryObserver(client, {
      ...boundedInfiniteListOptions,
      queryKey: ["bounded-pages"],
      queryFn: ({ pageParam }) => Promise.resolve(Number(pageParam)),
      initialPageParam: 0,
      getNextPageParam: (lastPage) => lastPage + 1,
    });
    const unsubscribe = observer.subscribe(() => undefined);

    await observer.refetch();
    for (let page = 1; page <= INFINITE_LIST_MAX_PAGES + 4; page += 1) await observer.fetchNextPage();

    expect(observer.getCurrentResult().data?.pages).toHaveLength(INFINITE_LIST_MAX_PAGES);
    expect(observer.getCurrentResult().data?.pages[0]).toBe(5);
    unsubscribe();
    client.clear();
  });
});
