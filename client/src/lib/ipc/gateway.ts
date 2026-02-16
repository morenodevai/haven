type EventHandler = (event: GatewayEvent) => void;

export interface GatewayEvent {
  type: string;
  data: any;
}

export class Gateway {
  private ws: WebSocket | null = null;
  private handlers: Map<string, EventHandler[]> = new Map();
  private reconnectTimer: number | null = null;
  private url: string;
  private token: string;

  constructor(url: string, token: string) {
    this.url = url;
    this.token = token;
  }

  connect() {
    this.ws = new WebSocket(this.url);

    this.ws.onopen = () => {
      // Send Identify command
      this.send({
        type: "Identify",
        data: { token: this.token },
      });
    };

    this.ws.onmessage = (event) => {
      try {
        const parsed: GatewayEvent = JSON.parse(event.data);
        this.emit(parsed.type, parsed);
      } catch {
        console.error("Failed to parse gateway event:", event.data);
      }
    };

    this.ws.onclose = () => {
      this.emit("Disconnected", { type: "Disconnected", data: {} });
      // Auto-reconnect after 3 seconds
      this.reconnectTimer = window.setTimeout(() => this.connect(), 3000);
    };

    this.ws.onerror = (err) => {
      console.error("Gateway error:", err);
    };
  }

  disconnect() {
    if (this.reconnectTimer) {
      clearTimeout(this.reconnectTimer);
      this.reconnectTimer = null;
    }
    this.ws?.close();
    this.ws = null;
  }

  send(data: object) {
    if (this.ws?.readyState === WebSocket.OPEN) {
      this.ws.send(JSON.stringify(data));
    }
  }

  on(event: string, handler: EventHandler) {
    if (!this.handlers.has(event)) {
      this.handlers.set(event, []);
    }
    this.handlers.get(event)!.push(handler);
  }

  off(event: string, handler: EventHandler) {
    const handlers = this.handlers.get(event);
    if (handlers) {
      const idx = handlers.indexOf(handler);
      if (idx !== -1) handlers.splice(idx, 1);
    }
  }

  private emit(event: string, data: GatewayEvent) {
    this.handlers.get(event)?.forEach((h) => h(data));
  }

  startTyping(channelId: string) {
    this.send({
      type: "StartTyping",
      data: { channel_id: channelId },
    });
  }
}
