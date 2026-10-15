import { QueryClient } from "@tanstack/react-query";

export const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      staleTime: 30_000,
      gcTime: 5 * 60_000,
      refetchOnWindowFocus: false,
      retry: 1,
    },
  },
});

// Search-as-you-type creates many short-lived keys; retaining them for the
// global five-minute window needlessly keeps every suggestion payload alive.
queryClient.setQueryDefaults(["search-suggest"], {
  staleTime: 5 * 60_000,
  gcTime: 60_000,
  retry: false,
});
queryClient.setQueryDefaults(["filter-facet-search"], {
  staleTime: 5 * 60_000,
  gcTime: 60_000,
  retry: false,
});

// Each search/filter combination owns an infinite-page cache. A short GC
// window prevents several fully scrolled large-library searches from being
// retained together after the user changes filters.
queryClient.setQueryDefaults(["library"], {
  staleTime: 30_000,
  gcTime: 2 * 60_000,
});

// Facets change only after a library mutation, where callers explicitly
// invalidate them. Avoid repeating their aggregate queries on ordinary tab
// navigation.
queryClient.setQueryDefaults(["library-facets"], {
  staleTime: 5 * 60_000,
  gcTime: 10 * 60_000,
});
queryClient.setQueryDefaults(["library-entities"], {
  staleTime: 60_000,
  gcTime: 2 * 60_000,
});
