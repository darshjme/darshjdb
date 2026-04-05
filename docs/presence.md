# Real-Time Presence

DarshJDB includes a built-in presence system for tracking online users, cursors, typing indicators, and arbitrary ephemeral state. Presence data is not persisted to the database -- it lives in memory on the server and is broadcast to connected clients in real time.

## Concepts

- **Room** -- A named channel that clients join. Presence is scoped per room.
- **Peer** -- A connected client within a room, identified by their connection ID.
- **Presence data** -- Arbitrary JSON each peer publishes. Other peers in the same room receive it instantly.

## Setting Presence

```typescript
// Join a room and set your presence
db.presence.enter('document-123', {
  name: 'Darsh',
  cursor: { x: 0, y: 0 },
  color: '#F59E0B',
});

// Update your presence (partial merge)
db.presence.update('document-123', {
  cursor: { x: 120, y: 340 },
});

// Leave the room
db.presence.leave('document-123');
```

Presence is automatically cleaned up when a client disconnects.

## Subscribing to Presence

### React

```tsx
import { usePresence } from '@darshjdb/react';

function CollaborativeEditor() {
  const { peers, myPresence, updatePresence } = usePresence('document-123', {
    name: currentUser.name,
    cursor: null,
    isTyping: false,
  });

  const handleMouseMove = (e: React.MouseEvent) => {
    updatePresence({ cursor: { x: e.clientX, y: e.clientY } });
  };

  return (
    <div onMouseMove={handleMouseMove}>
      {/* Render remote cursors */}
      {peers.map((peer) => (
        <RemoteCursor
          key={peer.id}
          name={peer.data.name}
          position={peer.data.cursor}
          color={peer.data.color}
        />
      ))}
    </div>
  );
}
```

### Vanilla TypeScript

```typescript
const room = db.presence.join('document-123', {
  name: 'Darsh',
  cursor: null,
});

// Listen for all presence changes
room.on('change', (peers) => {
  console.log('Current peers:', peers);
});

// Listen for specific events
room.on('join', (peer) => {
  console.log(`${peer.data.name} joined`);
});

room.on('leave', (peer) => {
  console.log(`${peer.data.name} left`);
});

room.on('update', (peer) => {
  console.log(`${peer.data.name} moved cursor to`, peer.data.cursor);
});

// Update your presence
room.update({ cursor: { x: 100, y: 200 } });

// Leave the room
room.leave();
```

## Typing Indicators

A common pattern built on top of presence:

```tsx
function ChatInput() {
  const { updatePresence } = usePresence('chat-room-1', {
    isTyping: false,
  });

  const handleInput = () => {
    updatePresence({ isTyping: true });
    // Debounce: clear typing after 2 seconds of inactivity
    clearTimeout(typingTimeout);
    typingTimeout = setTimeout(() => {
      updatePresence({ isTyping: false });
    }, 2000);
  };

  return <input onInput={handleInput} />;
}

function TypingIndicator() {
  const { peers } = usePresence('chat-room-1');
  const typing = peers.filter((p) => p.data.isTyping);

  if (typing.length === 0) return null;
  if (typing.length === 1) return <p>{typing[0].data.name} is typing...</p>;
  return <p>{typing.length} people are typing...</p>;
}
```

## Online Status

Track which users are currently online across your entire application:

```typescript
// Enter the global presence room on app load
db.presence.enter('app:online', {
  userId: currentUser.id,
  name: currentUser.name,
  lastSeen: Date.now(),
});

// Query who is online
const { peers } = usePresence('app:online');
const onlineUserIds = peers.map((p) => p.data.userId);
```

## REST API

Presence is primarily a WebSocket feature, but you can query current room state via the REST API:

```bash
# Get all peers in a room
curl http://localhost:7700/api/presence/document-123 \
  -H "Authorization: Bearer TOKEN"

# Response:
# {
#   "room": "document-123",
#   "peers": [
#     { "id": "conn-1", "data": { "name": "Darsh", "cursor": { "x": 120, "y": 340 } } },
#     { "id": "conn-2", "data": { "name": "Alex", "cursor": { "x": 50, "y": 100 } } }
#   ]
# }
```

## Configuration

| Variable | Default | Description |
|----------|---------|-------------|
| `DDB_PRESENCE_MAX_ROOMS` | `10000` | Maximum concurrent rooms |
| `DDB_PRESENCE_MAX_PEERS_PER_ROOM` | `500` | Maximum peers per room |
| `DDB_PRESENCE_HEARTBEAT_INTERVAL` | `30000` | Heartbeat interval in ms |
| `DDB_PRESENCE_TIMEOUT` | `60000` | Peer eviction timeout in ms |

## Permissions

Presence rooms respect the same authentication model as the rest of DarshJDB. Only authenticated users can join rooms by default. You can configure room-level access in your permissions file:

```typescript
// darshan/permissions.ts
export default {
  presence: {
    // Only allow authenticated users
    join: (ctx) => !!ctx.auth,

    // Or restrict to specific rooms
    join: (ctx, { room }) => {
      if (room.startsWith('admin:')) return ctx.auth.role === 'admin';
      return !!ctx.auth;
    },
  },
};
```

---

[Previous: Permissions](permissions.md) | [Next: Storage](storage.md) | [All Docs](README.md)
