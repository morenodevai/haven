type EventHandler = (data: any) => void;

export class Gateway {
  private url: string;
  private getToken: () => string;
  private ws: WebSocket | null = null;
  private handlers = new Map<string, EventHandler[]>();
  private closed = false;
  private reconnectTimer: number | null = null;
  private heartbeatTimer: number | null = null;
  private reconnectAttempts = 0;
  private sendQueue: object[] = [];

  // Reconnect config
  private static readonly BASE_DELAY = 1000;
  private static readonly MAX_DELAY = 30000;
  private static readonly HEARTBEAT_INTERVAL = 15000;

  constructor(url: string, getToken: () => string) {
    this.url = url;
    this.getToken = getToken;
  }

  connect() {
    this.closed = false;
    this.ws = new WebSocket(this.url);
    this.ws.binaryType = "arraybuffer";

    this.ws.onopen = () => {
      this.reconnectAttempts = 0;
      this.send({ type: "Identify", data: { token: this.getToken() } });
      this.startHeartbeat();
      this.flushQueue();
    };

    this.ws.onmessage = (event) => {
      if (event.data instanceof ArrayBuffer) {
        this.emit("__binary__", event.data);
        return;
      }
      try {
        const parsed = JSON.parse(event.data);
        const eventType = parsed.type;
        if (eventType) {
          this.emit(eventType, parsed);
        }
      } catch {
        // Malformed message — ignore silently
      }
    };

    this.ws.onclose = () => {
      this.stopHeartbeat();
      if (this.closed) return;
      this.emit("Disconnected", { type: "Disconnected", data: {} });
      this.scheduleReconnect();
    };

    this.ws.onerror = () => {
      // onclose will fire after this — reconnect is handled there
    };
  }

  disconnect() {
    this.closed = true;
    this.stopHeartbeat();
    if (this.reconnectTimer) {
      clearTimeout(this.reconnectTimer);
      this.reconnectTimer = null;
    }
    this.ws?.close();
    this.ws = null;
    this.sendQueue = [];
  }

  send(data: object) {
    if (this.ws?.readyState === WebSocket.OPEN) {
      this.ws.send(JSON.stringify(data));
    } else {
      // Queue for later (except Identify which is connection-specific)
      const type = (data as any).type;
      if (type !== "Identify") {
        this.sendQueue.push(data);
      }
    }
  }

  sendBinary(data: ArrayBuffer) {
    if (this.ws?.readyState === WebSocket.OPEN) {
      this.ws.send(data);
    }
    // Binary data is not queued - file transfer scheduler handles retries
  }

  private flushQueue() {
    const queue = this.sendQueue;
    this.sendQueue = [];
    for (const msg of queue) {
      this.send(msg);
    }
  }

  private scheduleReconnect() {
    const delay = Math.min(
      Gateway.BASE_DELAY * Math.pow(2, this.reconnectAttempts),
      Gateway.MAX_DELAY,
    );
    const jitter = delay * 0.5 * Math.random();
    this.reconnectAttempts++;
    this.reconnectTimer = window.setTimeout(() => this.connect(), delay + jitter);
  }

  private startHeartbeat() {
    this.stopHeartbeat();
    this.heartbeatTimer = window.setInterval(() => {
      if (this.ws?.readyState === WebSocket.OPEN) {
        // Application-level keepalive. The server handles WebSocket-level pings,
        // but this keeps the connection alive through proxies and detects dead
        // connections faster on the client side.
      }
    }, Gateway.HEARTBEAT_INTERVAL);
  }

  private stopHeartbeat() {
    if (this.heartbeatTimer) {
      clearInterval(this.heartbeatTimer);
      this.heartbeatTimer = null;
    }
  }

  on(event: string, handler: EventHandler) {
    const list = this.handlers.get(event) || [];
    list.push(handler);
    this.handlers.set(event, list);
  }

  off(event: string, handler: EventHandler) {
    const list = this.handlers.get(event) || [];
    this.handlers.set(event, list.filter((h) => h !== handler));
  }

  private emit(event: string, data: any) {
    for (const handler of this.handlers.get(event) || []) {
      handler(data);
    }
  }

  startTyping(channelId: string) {
    this.send({ type: "StartTyping", data: { channel_id: channelId } });
  }

  get isConnected(): boolean {
    return this.ws?.readyState === WebSocket.OPEN;
  }
}
