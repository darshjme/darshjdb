import React, { useState, useRef, useEffect } from "react";
import { useQuery, useMutation, usePresence, useAuth } from "@darshjdb/react";

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

interface PresenceState {
  name: string;
  typing: boolean;
}

// ---------------------------------------------------------------------------
// Auth Gate
// ---------------------------------------------------------------------------

function AuthGate({ children }: { children: React.ReactNode }) {
  const { user, isLoading, signIn, signUp, signOut, error } = useAuth();
  const [email, setEmail] = useState("demo@example.com");
  const [password, setPassword] = useState("demo1234");
  const [displayName, setDisplayName] = useState("Demo User");
  const [mode, setMode] = useState<"signin" | "signup">("signin");
  const [authError, setAuthError] = useState<string | null>(null);

  if (isLoading) {
    return <p style={{ textAlign: "center", padding: 40 }}>Authenticating...</p>;
  }

  if (user) {
    return (
      <div>
        <header style={headerStyle}>
          <h1 style={{ margin: 0, fontSize: 18 }}>DarshJDB Chat</h1>
          <div style={{ display: "flex", alignItems: "center", gap: 12 }}>
            <span style={{ fontSize: 14, color: "#666" }}>
              {user.displayName ?? user.email}
            </span>
            <button onClick={signOut} style={smallButtonStyle}>
              Sign Out
            </button>
          </div>
        </header>
        {children}
      </div>
    );
  }

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    setAuthError(null);
    try {
      if (mode === "signup") {
        await signUp({ email, password, displayName });
      } else {
        await signIn({ email, password });
      }
    } catch (err) {
      setAuthError(err instanceof Error ? err.message : "Authentication failed");
    }
  };

  return (
    <div style={{ maxWidth: 360, margin: "80px auto", fontFamily: "system-ui" }}>
      <h1 style={{ marginBottom: 24 }}>DarshJDB Chat</h1>
      <form onSubmit={handleSubmit} style={{ display: "flex", flexDirection: "column", gap: 12 }}>
        {mode === "signup" && (
          <input
            value={displayName}
            onChange={(e) => setDisplayName(e.target.value)}
            placeholder="Display name"
            style={inputStyle}
          />
        )}
        <input
          value={email}
          onChange={(e) => setEmail(e.target.value)}
          placeholder="Email"
          type="email"
          required
          style={inputStyle}
        />
        <input
          value={password}
          onChange={(e) => setPassword(e.target.value)}
          placeholder="Password"
          type="password"
          required
          style={inputStyle}
        />
        {(authError || error) && (
          <p style={{ color: "#c00", fontSize: 14, margin: 0 }}>
            {authError ?? error?.message}
          </p>
        )}
        <button type="submit" style={primaryButtonStyle}>
          {mode === "signin" ? "Sign In" : "Create Account"}
        </button>
        <button
          type="button"
          onClick={() => setMode(mode === "signin" ? "signup" : "signin")}
          style={{ ...smallButtonStyle, alignSelf: "center" }}
        >
          {mode === "signin" ? "Need an account? Sign up" : "Have an account? Sign in"}
        </button>
      </form>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Chat Room
// ---------------------------------------------------------------------------

function ChatRoom() {
  const { user } = useAuth();
  const [message, setMessage] = useState("");
  const listRef = useRef<HTMLDivElement>(null);
  const typingTimeoutRef = useRef<ReturnType<typeof setTimeout>>();

  const userName = user?.displayName ?? user?.email ?? "Anonymous";

  // Live query -- messages update in real time across all connected clients
  const { data, isLoading } = useQuery({
    collection: "messages",
    orderBy: [{ field: "createdAt", direction: "asc" }],
    limit: 100,
  });

  // Presence -- see who is online and who is typing
  const { peers, publishState } = usePresence<PresenceState>("chat-room");

  // Publish our presence on mount
  useEffect(() => {
    publishState({ name: userName, typing: false });
  }, [publishState, userName]);

  // Mutation for sending messages
  const { mutate, isLoading: isSending } = useMutation();

  // Auto-scroll to bottom when new messages arrive
  const messages = data ?? [];
  useEffect(() => {
    if (listRef.current) {
      listRef.current.scrollTop = listRef.current.scrollHeight;
    }
  }, [messages.length]);

  const handleTyping = () => {
    publishState({ name: userName, typing: true });
    clearTimeout(typingTimeoutRef.current);
    typingTimeoutRef.current = setTimeout(() => {
      publishState({ name: userName, typing: false });
    }, 1500);
  };

  const handleSend = async (e: React.FormEvent) => {
    e.preventDefault();
    const text = message.trim();
    if (!text) return;

    setMessage("");
    publishState({ name: userName, typing: false });

    await mutate({
      type: "insert",
      collection: "messages",
      data: {
        text,
        sender: userName,
        senderId: user?.id,
        createdAt: Date.now(),
      },
    });
  };

  const typingPeers = peers
    .filter((p) => p.state.typing)
    .map((p) => p.state.name);

  if (isLoading) {
    return <p style={{ padding: 24, textAlign: "center" }}>Loading messages...</p>;
  }

  return (
    <div style={{ display: "flex", height: "calc(100vh - 56px)", fontFamily: "system-ui" }}>
      {/* Message list */}
      <div style={{ flex: 1, display: "flex", flexDirection: "column" }}>
        <div ref={listRef} style={messageListStyle}>
          {messages.length === 0 && (
            <p style={{ color: "#999", textAlign: "center", padding: 40 }}>
              No messages yet. Say something!
            </p>
          )}
          {messages.map((msg: any) => {
            const isOwn = msg.senderId === user?.id;
            return (
              <div
                key={msg.id}
                style={{
                  display: "flex",
                  flexDirection: "column",
                  alignItems: isOwn ? "flex-end" : "flex-start",
                  marginBottom: 8,
                }}
              >
                <span style={{ fontSize: 12, color: "#999", marginBottom: 2 }}>
                  {msg.sender}
                </span>
                <div
                  style={{
                    background: isOwn ? "#000" : "#f0f0f0",
                    color: isOwn ? "#fff" : "#000",
                    padding: "8px 14px",
                    borderRadius: 16,
                    maxWidth: "70%",
                    wordBreak: "break-word",
                  }}
                >
                  {msg.text}
                </div>
              </div>
            );
          })}
        </div>

        {/* Typing indicator */}
        <div style={{ height: 24, padding: "0 16px", fontSize: 13, color: "#999" }}>
          {typingPeers.length > 0 && (
            <span>
              {typingPeers.join(", ")} {typingPeers.length === 1 ? "is" : "are"} typing...
            </span>
          )}
        </div>

        {/* Message input */}
        <form onSubmit={handleSend} style={inputBarStyle}>
          <input
            value={message}
            onChange={(e) => {
              setMessage(e.target.value);
              handleTyping();
            }}
            placeholder="Type a message..."
            style={{ ...inputStyle, flex: 1 }}
            autoFocus
          />
          <button
            type="submit"
            disabled={isSending || !message.trim()}
            style={primaryButtonStyle}
          >
            Send
          </button>
        </form>
      </div>

      {/* Online users sidebar */}
      <aside style={sidebarStyle}>
        <h3 style={{ margin: "0 0 12px", fontSize: 14, color: "#666" }}>
          Online ({peers.length + 1})
        </h3>
        <div style={peerStyle}>
          <span style={dotStyle("#22c55e")} />
          {userName} (you)
        </div>
        {peers.map((peer) => (
          <div key={peer.peerId} style={peerStyle}>
            <span style={dotStyle("#22c55e")} />
            {peer.state.name}
            {peer.state.typing && (
              <span style={{ color: "#999", fontSize: 12, marginLeft: 4 }}>
                typing...
              </span>
            )}
          </div>
        ))}
      </aside>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Root
// ---------------------------------------------------------------------------

export function App() {
  return (
    <AuthGate>
      <ChatRoom />
    </AuthGate>
  );
}

// ---------------------------------------------------------------------------
// Styles
// ---------------------------------------------------------------------------

const headerStyle: React.CSSProperties = {
  display: "flex",
  justifyContent: "space-between",
  alignItems: "center",
  padding: "12px 16px",
  borderBottom: "1px solid #eee",
  fontFamily: "system-ui",
  height: 56,
};

const messageListStyle: React.CSSProperties = {
  flex: 1,
  overflowY: "auto",
  padding: 16,
};

const inputBarStyle: React.CSSProperties = {
  display: "flex",
  gap: 8,
  padding: "12px 16px",
  borderTop: "1px solid #eee",
};

const sidebarStyle: React.CSSProperties = {
  width: 200,
  borderLeft: "1px solid #eee",
  padding: 16,
  overflowY: "auto",
};

const peerStyle: React.CSSProperties = {
  display: "flex",
  alignItems: "center",
  gap: 8,
  padding: "6px 0",
  fontSize: 14,
};

const dotStyle = (color: string): React.CSSProperties => ({
  width: 8,
  height: 8,
  borderRadius: "50%",
  background: color,
  flexShrink: 0,
});

const inputStyle: React.CSSProperties = {
  padding: "10px 14px",
  fontSize: 15,
  borderRadius: 8,
  border: "1px solid #ddd",
  outline: "none",
};

const primaryButtonStyle: React.CSSProperties = {
  padding: "10px 20px",
  fontSize: 15,
  borderRadius: 8,
  background: "#000",
  color: "#fff",
  border: "none",
  cursor: "pointer",
};

const smallButtonStyle: React.CSSProperties = {
  padding: "4px 12px",
  fontSize: 13,
  borderRadius: 6,
  background: "none",
  border: "1px solid #ddd",
  cursor: "pointer",
  color: "#666",
};
