<script lang="ts">
  import { auth } from "./lib/stores/auth";
  import { channelKey, loadMessages, handleIncomingMessage } from "./lib/stores/messages";
  import { Gateway } from "./lib/ipc/gateway";
  import { getBaseUrl } from "./lib/ipc/api";
  import Login from "./lib/components/auth/Login.svelte";
  import Sidebar from "./lib/components/sidebar/Sidebar.svelte";
  import MessageList from "./lib/components/chat/MessageList.svelte";
  import MessageInput from "./lib/components/chat/MessageInput.svelte";

  let gateway: Gateway | null = $state(null);
  let connected = $state(false);

  // Connect to gateway when logged in + have channel key
  $effect(() => {
    if ($auth.loggedIn && $auth.token && $channelKey) {
      // Load message history
      loadMessages();

      // Connect WebSocket
      const wsUrl = getBaseUrl().replace("http", "ws") + "/gateway";
      const gw = new Gateway(wsUrl, $auth.token);

      gw.on("Ready", () => {
        connected = true;
      });

      gw.on("MessageCreate", (event) => {
        handleIncomingMessage(event);
      });

      gw.on("Disconnected", () => {
        connected = false;
      });

      gw.connect();
      gateway = gw;

      return () => {
        gw.disconnect();
        gateway = null;
        connected = false;
      };
    }
  });
</script>

{#if !$auth.loggedIn || !$channelKey}
  <Login />
{:else}
  <div class="app-layout">
    <Sidebar />
    <div class="main-content">
      <div class="channel-header">
        <span class="hash">#</span>
        <span class="channel-name">general</span>
        <div class="status" class:online={connected}>
          {connected ? "Connected" : "Connecting..."}
        </div>
      </div>
      <MessageList />
      <MessageInput />
    </div>
  </div>
{/if}

<style>
  .app-layout {
    display: flex;
    height: 100%;
  }

  .main-content {
    flex: 1;
    display: flex;
    flex-direction: column;
    min-width: 0;
  }

  .channel-header {
    padding: 12px 16px;
    border-bottom: 1px solid var(--border);
    display: flex;
    align-items: center;
    gap: 6px;
    flex-shrink: 0;
  }

  .hash {
    color: var(--text-muted);
    font-size: 20px;
    font-weight: 600;
  }

  .channel-name {
    font-weight: 700;
    font-size: 16px;
  }

  .status {
    margin-left: auto;
    font-size: 12px;
    color: var(--text-muted);
    display: flex;
    align-items: center;
    gap: 6px;
  }

  .status::before {
    content: "";
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--error);
  }

  .status.online::before {
    background: var(--success);
  }
</style>
