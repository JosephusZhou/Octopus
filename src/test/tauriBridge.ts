/** Deterministic webview IPC shim used by component tests (see src/test/setup.ts). */
type Handler = (payload: unknown) => void;

export interface TauriBridge {
  invoke: ((cmd: string, payload: Record<string, unknown>) => Promise<unknown>) | null;
  calls: Array<{ cmd: string; payload: Record<string, unknown> }>;
  commands(): string[];
  emit(event: string, payload?: unknown): void;
  reset(): void;
}

const listeners = new Map<number, { event: string; handler: Handler }>();
const callbacks = new Map<number, Handler>();
let nextCallbackId = 1;
let nextEventId = 1;

export const bridge: TauriBridge = {
  invoke: null,
  calls: [],
  commands() {
    return this.calls.map((call) => call.cmd);
  },
  emit(event, payload) {
    for (const entry of listeners.values()) {
      if (entry.event === event) entry.handler({ event, payload });
    }
  },
  reset() {
    this.invoke = null;
    this.calls.length = 0;
    listeners.clear();
    callbacks.clear();
  },
};

export function installTauriBridge() {
  Object.defineProperty(window, "__TAURI_INTERNALS__", {
    configurable: true,
    value: {
      async invoke(cmd: string, payload: Record<string, unknown> = {}) {
        bridge.calls.push({ cmd, payload });
        if (cmd === "plugin:event|listen") {
          const id = payload.handler as number;
          const eventId = nextEventId++;
          listeners.set(eventId, { event: payload.event as string, handler: callbacks.get(id)! });
          return eventId;
        }
        if (cmd === "plugin:event|unlisten") {
          listeners.delete(payload.eventId as number);
          return null;
        }
        if (!bridge.invoke) throw new Error(`No test invoke handler for command: ${cmd}`);
        return bridge.invoke(cmd, payload);
      },
      transformCallback(handler: Handler) {
        const id = nextCallbackId++;
        callbacks.set(id, handler);
        return id;
      },
      unregisterCallback(id: number) {
        callbacks.delete(id);
      },
    },
  });

  Object.defineProperty(window, "__TAURI_EVENT_PLUGIN_INTERNALS__", {
    configurable: true,
    value: { registerListener() {}, unregisterListener() {} },
  });
}
