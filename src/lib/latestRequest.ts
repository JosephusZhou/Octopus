/** A request may apply results only while it is the newest request in its scope. */
export function createLatestRequest() {
  let version = 0;
  let scope: unknown;
  let active = true;
  return {
    setScope(next: unknown) {
      if (!Object.is(scope, next)) {
        scope = next;
        version += 1;
      }
    },
    invalidate() {
      version += 1;
    },
    activate() {
      active = true;
    },
    // React StrictMode remounts the same instance: stop callbacks while detached,
    // but keep in-flight requests applicable after re-activation. A real unmount
    // never re-activates, so its callbacks are dropped forever.
    dispose() {
      active = false;
    },
    begin() {
      const ticket = ++version;
      return () => active && ticket === version;
    },
  };
}
