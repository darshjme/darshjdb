# DarshJDB Chat App

A real-time chat application built with React and DarshJDB. Demonstrates live queries, mutations, presence tracking, and authentication -- all the building blocks for collaborative apps.

## What it demonstrates

- **Real-time messages** via live queries -- new messages appear instantly for all users
- **Presence** -- see who is online and who is currently typing
- **Authentication** -- sign up / sign in flow using `useAuth`
- **Mutations** with optimistic updates for instant send feedback
- **Multi-user** -- open in two browser tabs to see real-time sync

## Prerequisites

- Node.js 18+
- A running DarshJDB server (default: `http://localhost:7700`)

## Setup

```bash
# From the repository root
npm install

# Start the dev server
cd examples/chat-app
npm run dev
```

The app runs at [http://localhost:3002](http://localhost:3002).

Open a second browser tab (or incognito window with a different account) to test the real-time features.

## Project structure

```
chat-app/
  index.html          Entry HTML shell
  package.json        Dependencies and scripts
  tsconfig.json       TypeScript config
  vite.config.ts      Vite dev server config (port 3002)
  src/
    main.tsx          App bootstrap with DarshanProvider
    App.tsx           Auth gate, chat room, presence sidebar
```

## Key code

### Presence tracking

```tsx
const { peers, publishState } = usePresence<{ name: string; typing: boolean }>("chat-room");

// Publish typing state
publishState({ name: "Alice", typing: true });

// Read who is online
peers.map(p => p.state.name);
```

### Live message query

```tsx
const { data } = useQuery({
  collection: "messages",
  orderBy: [{ field: "createdAt", direction: "asc" }],
  limit: 100,
});
```

### Sending a message

```tsx
const { mutate } = useMutation();

await mutate({
  type: "insert",
  collection: "messages",
  data: { text: "Hello!", sender: "Alice", createdAt: Date.now() },
});
```

### Authentication

```tsx
const { user, signIn, signUp, signOut } = useAuth();

await signIn({ email: "demo@example.com", password: "demo1234" });
```
