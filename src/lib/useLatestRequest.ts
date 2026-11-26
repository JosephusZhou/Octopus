import { useEffect, useRef } from "react";
import { createLatestRequest } from "./latestRequest";

/** Stable request gate; changing the scope or unmounting invalidates old results. */
export function useLatestRequest(scope?: unknown) {
  const gate = useRef<ReturnType<typeof createLatestRequest>>();
  if (!gate.current) gate.current = createLatestRequest();
  gate.current.setScope(scope);
  const current = gate.current;
  useEffect(() => {
    current.activate();
    return () => current.dispose();
  }, [current]);
  return current;
}
