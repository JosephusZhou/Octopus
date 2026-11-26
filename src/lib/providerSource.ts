import { useEffect } from "react";

export interface ProviderSourceView {
  id: string;
  name: string;
  models: Array<{ id: string; original: string }>;
}
export async function getProviderSource(
  sourceId: string,
  protocol: string,
): Promise<ProviderSourceView> {
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke("get_provider_source_models", { sourceId, protocol });
}
/** Refresh consumers when Rust finishes applying source changes; clean up even if import resolves late. */
export function useProviderSourceChanges(refresh: () => unknown): void {
  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void import("@tauri-apps/api/event")
      .then(async ({ listen }) => {
        const cleanup = await listen("provider-sources-changed", () => {
          if (!disposed)
            void Promise.resolve()
              .then(() => {
                if (!disposed) return refresh();
              })
              .catch(() => {});
        });
        if (disposed) cleanup();
        else unlisten = cleanup;
      })
      .catch(() => {
        /* browser-only renderer has no native events */
      });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [refresh]);
}
