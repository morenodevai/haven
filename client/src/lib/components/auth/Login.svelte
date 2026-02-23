<script lang="ts">
  import { login, register, authError, rememberMe, saveRememberMe, tryAutoLogin, autoLoginInProgress, skipAutoLogin } from "../../stores/auth";
  import { generateKey } from "../../ipc/crypto";
  import { setChannelKey, DEFAULT_CHANNEL_KEY } from "../../stores/messages";
  import { onMount } from "svelte";

  let username = $state("");
  let password = $state("");
  let channelKeyInput = $state("");
  let isRegistering = $state(false);
  let loading = $state(false);
  let showKeySetup = $state(false);
  let remember = $state(true);

  // Attempt auto-login from saved credentials on mount
  onMount(() => {
    tryAutoLogin().then((success) => {
      if (success) {
        // Check if channel key was restored
        const savedKey = localStorage.getItem("haven_channel_key");
        if (!savedKey) {
          channelKeyInput = DEFAULT_CHANNEL_KEY;
          showKeySetup = true;
        }
      }
    });
  });

  async function handleSubmit() {
    if (!username || !password) return;
    loading = true;

    try {
      if (isRegistering) {
        await register(username, password);
      } else {
        await login(username, password);
      }

      // Check if we have a channel key
      const savedKey = localStorage.getItem("haven_channel_key");
      if (!savedKey) {
        channelKeyInput = DEFAULT_CHANNEL_KEY;
        showKeySetup = true;
        loading = false;
        return;
      }

      // Save credentials if Remember Me is checked
      if (remember) {
        await saveRememberMe(username, password, savedKey);
        rememberMe.set(true);
      }
    } catch {
      // Error already set in store
    }

    loading = false;
  }

  async function handleGenerateKey() {
    const key = await generateKey();
    channelKeyInput = key;
  }

  async function handleSetKey() {
    if (!channelKeyInput.trim()) return;
    setChannelKey(channelKeyInput.trim());
    showKeySetup = false;

    // Save credentials + channel key if Remember Me was checked
    if (remember) {
      await saveRememberMe(username, password, channelKeyInput.trim());
      rememberMe.set(true);
    }
  }
</script>

{#if $autoLoginInProgress}
  <div class="login-container">
    <div class="login-card">
      <div class="logo">H</div>
      <h1>Haven</h1>
      <p class="subtitle">Signing in...</p>
      <div class="auto-login-spinner"></div>
      <button class="toggle" onclick={skipAutoLogin}>
        Log into a different account
      </button>
    </div>
  </div>
{:else if showKeySetup}
  <div class="login-container">
    <div class="login-card">
      <div class="logo key-logo">K</div>
      <h1>Channel Key</h1>
      <p class="subtitle">
        Enter a shared encryption key, or generate a new one and share it with your friend.
      </p>

      <div class="form">
        <input
          type="text"
          placeholder="Paste encryption key here..."
          bind:value={channelKeyInput}
          class="key-input"
        />

        <button class="btn primary" onclick={handleSetKey}>
          Use This Key
        </button>

        <button class="btn secondary" onclick={handleGenerateKey}>
          Generate New Key
        </button>

        {#if channelKeyInput}
          <p class="key-warning">
            Share this key with your friend through a secure channel (in person, Signal, etc).
            Anyone with this key can read your messages.
          </p>
        {/if}
      </div>
    </div>
  </div>
{:else}
  <div class="login-container">
    <div class="login-card">
      <div class="logo">H</div>
      <h1>Haven</h1>
      <p class="subtitle">
        {isRegistering ? "Create your account" : "Welcome back"}
      </p>

      {#if $authError}
        <div class="error">{$authError}</div>
      {/if}

      <form class="form" onsubmit={(e) => { e.preventDefault(); handleSubmit(); }}>
        <input
          type="text"
          placeholder="Username"
          bind:value={username}
          autocomplete="username"
          minlength="3"
          maxlength="32"
          required
        />

        <input
          type="password"
          placeholder="Password"
          bind:value={password}
          autocomplete={isRegistering ? "new-password" : "current-password"}
          minlength="8"
          required
        />

        {#if !isRegistering}
          <label class="remember-me">
            <input type="checkbox" bind:checked={remember} />
            <span>Remember me</span>
          </label>
        {/if}

        <button class="btn primary" type="submit" disabled={loading}>
          {loading ? "..." : isRegistering ? "Create Account" : "Log In"}
        </button>
      </form>

      <button class="toggle" onclick={() => { isRegistering = !isRegistering; authError.set(null); }}>
        {isRegistering ? "Already have an account? Log in" : "Need an account? Register"}
      </button>
    </div>
  </div>
{/if}

<style>
  .login-container {
    display: flex;
    align-items: center;
    justify-content: center;
    height: 100%;
    background: var(--bg-primary);
  }

  .login-card {
    background: var(--bg-secondary);
    border: 1px solid var(--border);
    border-radius: 12px;
    padding: 40px;
    width: 380px;
    text-align: center;
  }

  .logo {
    width: 64px;
    height: 64px;
    background: var(--accent);
    border-radius: 16px;
    display: flex;
    align-items: center;
    justify-content: center;
    font-size: 28px;
    font-weight: 700;
    color: white;
    margin: 0 auto 16px;
  }

  h1 {
    font-size: 24px;
    margin-bottom: 4px;
  }

  .subtitle {
    color: var(--text-secondary);
    margin-bottom: 24px;
    font-size: 13px;
  }

  .form {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  input {
    background: var(--bg-input);
    border: 1px solid var(--border);
    border-radius: 8px;
    padding: 12px 14px;
    color: var(--text-primary);
    outline: none;
    transition: border-color 0.2s;
  }

  input:focus {
    border-color: var(--accent);
  }

  .key-input {
    font-family: monospace;
    font-size: 11px;
    word-break: break-all;
  }

  .btn {
    padding: 12px;
    border-radius: 8px;
    border: none;
    font-weight: 600;
    transition: background-color 0.2s;
  }

  .btn.primary {
    background: var(--accent);
    color: white;
  }

  .btn.primary:hover:not(:disabled) {
    background: var(--accent-hover);
  }

  .btn.primary:disabled {
    opacity: 0.5;
  }

  .btn.secondary {
    background: var(--bg-input);
    color: var(--text-secondary);
    border: 1px solid var(--border);
  }

  .btn.secondary:hover {
    background: var(--border);
  }

  .toggle {
    background: none;
    border: none;
    color: var(--accent);
    margin-top: 16px;
    font-size: 13px;
  }

  .toggle:hover {
    text-decoration: underline;
  }

  .error {
    background: rgba(239, 68, 68, 0.1);
    border: 1px solid var(--error);
    color: var(--error);
    border-radius: 8px;
    padding: 10px;
    margin-bottom: 12px;
    font-size: 13px;
  }

  .key-warning {
    color: var(--text-muted);
    font-size: 12px;
    line-height: 1.4;
    padding: 8px;
    background: rgba(108, 99, 255, 0.08);
    border-radius: 6px;
  }

  .key-logo {
    background: var(--accent);
  }

  .remember-me {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 13px;
    color: var(--text-secondary);
    cursor: pointer;
    user-select: none;
  }

  .remember-me input[type="checkbox"] {
    width: 16px;
    height: 16px;
    accent-color: var(--accent);
    cursor: pointer;
  }

  .auto-login-spinner {
    width: 32px;
    height: 32px;
    border: 3px solid var(--border);
    border-top-color: var(--accent);
    border-radius: 50%;
    margin: 16px auto 0;
    animation: spin 0.8s linear infinite;
  }

  @keyframes spin {
    from { transform: rotate(0deg); }
    to { transform: rotate(360deg); }
  }
</style>
