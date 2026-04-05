# DarshJDB Web3 Strategy

**Making DarshJDB the default backend for decentralized applications.**

> DarshJDB already solves the hardest problems in backend engineering: real-time sync, offline-first, permissions, server functions, and single-binary deployment. Web3 dApps need all of this *plus* wallet auth, on-chain data, token-gated access, and verifiable state. This document lays out how DarshJDB becomes the self-hosted Firebase replacement that Web3 has been waiting for.

---

## Table of Contents

1. [Wallet Authentication](#1-wallet-authentication)
2. [On-Chain Data Sync](#2-on-chain-data-sync)
3. [Decentralized Storage Integration](#3-decentralized-storage-integration)
4. [Token-Gated Permissions](#4-token-gated-permissions)
5. [Verifiable Data](#5-verifiable-data)
6. [dApp Backend Pattern](#6-dapp-backend-pattern)
7. [Implementation Roadmap](#7-implementation-roadmap)

---

## 1. Wallet Authentication

### Problem

dApp users do not have email/password credentials. Their identity *is* their wallet. The standard authentication flow (Argon2id hashed passwords, OAuth redirects) is irrelevant. A wallet signature is cryptographic proof of identity that is stronger than any password.

### Design

DarshJDB's Auth Engine currently supports email/password, magic links, OAuth, MFA, and WebAuthn. Wallet auth slots in as a new auth provider at the same level as OAuth, reusing the existing JWT issuance, refresh token rotation, and session management infrastructure.

#### Sign-In with Ethereum (SIWE / EIP-4361)

The SIWE standard defines a human-readable message format that the user signs with their Ethereum private key. The server verifies the signature, extracts the wallet address, and issues a DarshJDB session.

**Flow:**

```
Client                          DarshJDB
  |                                |
  |-- GET /auth/wallet/nonce ----->|  Generate random nonce, store with expiry
  |<---- { nonce, expiresAt } -----|
  |                                |
  |  User signs SIWE message       |
  |  in MetaMask / WalletConnect   |
  |                                |
  |-- POST /auth/wallet/verify --->|  Verify EIP-191 signature
  |   { message, signature }       |  Extract address from recovery
  |                                |  Find or create user by address
  |                                |  Issue JWT + refresh token
  |<---- { accessToken,           |
  |        refreshToken, user } ---|
```

**Server-side verification (Rust pseudocode):**

```rust
use alloy_primitives::Address;
use alloy_signer::SignerSync;

pub async fn verify_siwe(
    message: &str,
    signature: &[u8; 65],
) -> Result<Address, AuthError> {
    // Parse the SIWE message (EIP-4361 format)
    let siwe_message: siwe::Message = message.parse()
        .map_err(|_| AuthError::InvalidMessage)?;

    // Verify the message hasn't expired and nonce matches
    siwe_message.verify(signature, &VerificationOpts {
        domain: Some("your-dapp.com".parse().unwrap()),
        nonce: Some(stored_nonce),
        timestamp: Some(Utc::now()),
    }).await.map_err(|_| AuthError::InvalidSignature)?;

    Ok(siwe_message.address)
}
```

**Client SDK usage:**

```typescript
import { DarshJDB } from '@darshjdb/react';
import { useAccount, useSignMessage } from 'wagmi';

const db = DarshJDB.init({ appId: 'my-dapp' });

function WalletLogin() {
  const { address } = useAccount();
  const { signMessageAsync } = useSignMessage();

  const login = async () => {
    // 1. Get nonce from DarshJDB
    const { nonce } = await db.auth.wallet.getNonce();

    // 2. Build SIWE message
    const message = db.auth.wallet.buildSiweMessage({
      address,
      nonce,
      statement: 'Sign in to MyDApp',
      chainId: 1,
    });

    // 3. Sign with wallet
    const signature = await signMessageAsync({ message });

    // 4. Verify and get session
    const session = await db.auth.wallet.verify({ message, signature });
    // session = { accessToken, refreshToken, user }
    // WebSocket auto-reconnects with new auth
  };

  return <button onClick={login}>Sign In with Ethereum</button>;
}
```

#### Sign-In with Solana

Same pattern, different cryptography. Solana uses Ed25519 instead of secp256k1. DarshJDB already uses Ed25519 for JWT signing, so the verification primitives are already in the codebase.

```typescript
const loginSolana = async () => {
  const { nonce } = await db.auth.wallet.getNonce();

  const message = db.auth.wallet.buildSolanaMessage({
    publicKey: wallet.publicKey.toBase58(),
    nonce,
    statement: 'Sign in to MyDApp',
  });

  const encodedMessage = new TextEncoder().encode(message);
  const signature = await wallet.signMessage(encodedMessage);

  const session = await db.auth.wallet.verify({
    message,
    signature: bs58.encode(signature),
    chain: 'solana',
  });
};
```

#### Wallet-Based Sessions

Once verified, wallet auth produces the same `{ accessToken, refreshToken, user }` tuple as every other auth method. The WebSocket connection upgrades seamlessly. No special handling needed downstream -- the Permission Engine, Role Resolution, and RLS all work identically because they operate on `user.id`, not on the auth method.

Key difference: wallet users have no email by default. The `user` record stores:

```sql
-- users table extension for wallet auth
ALTER TABLE users ADD COLUMN wallet_address TEXT UNIQUE;
ALTER TABLE users ADD COLUMN wallet_chain TEXT; -- 'ethereum' | 'solana' | 'cosmos'
ALTER TABLE users ADD COLUMN wallet_linked_at TIMESTAMPTZ;
```

#### Multi-Chain Identity Linking

A single DarshJDB user can link multiple wallets across chains. This mirrors how OAuth lets you link Google + GitHub to one account.

```typescript
// Already logged in with Ethereum wallet
// Now link a Solana wallet to the same account
await db.auth.wallet.link({
  chain: 'solana',
  publicKey: solanaWallet.publicKey.toBase58(),
  signature: await solanaWallet.signMessage(linkMessage),
});

// Query user's linked wallets
const { data } = db.useQuery({
  users: {
    $where: { id: db.auth.userId },
    wallets: {} // linked wallets as a relation
  }
});
```

**Identity resolution rules:**
- Same wallet address always resolves to same user (even across sessions)
- Linking requires active session + wallet signature (proves ownership of both)
- Unlinking requires re-authentication
- Primary wallet cannot be unlinked if no other auth method exists

---

## 2. On-Chain Data Sync

### Problem

dApps need their off-chain database to reflect on-chain state. Currently, developers run separate indexing infrastructure (The Graph, custom node scripts, Alchemy webhooks) and manually sync data into their database. This is fragile, slow, and duplicates work DarshJDB could handle natively.

### Design

DarshJDB's Sync Engine already watches for database mutations and pushes diffs to connected clients. On-chain data sync extends this in the opposite direction: DarshJDB watches blockchain events and writes them into the triple store, triggering the same reactive query pipeline.

#### Event Listener Framework

A new `ChainListener` service runs alongside the existing services. It maintains WebSocket/HTTP connections to RPC providers and filters for relevant events.

**Configuration (ddb.config.ts):**

```typescript
import { defineConfig } from '@darshjdb/server';

export default defineConfig({
  chains: {
    ethereum: {
      rpc: process.env.ETH_RPC_URL, // Alchemy, Infura, or self-hosted
      chainId: 1,
      listeners: [
        {
          // Listen to ERC-721 Transfer events on a specific contract
          contract: '0x1234...abcd',
          abi: nftAbi,
          event: 'Transfer',
          handler: 'functions/onNftTransfer.ts',
          fromBlock: 'latest', // or specific block number for backfill
        },
        {
          // Listen to all USDC transfers above 10k
          contract: USDC_ADDRESS,
          abi: erc20Abi,
          event: 'Transfer',
          filter: (log) => BigInt(log.args.value) > parseUnits('10000', 6),
          handler: 'functions/onLargeTransfer.ts',
        },
      ],
    },
    polygon: {
      rpc: process.env.POLYGON_RPC_URL,
      chainId: 137,
      listeners: [/* ... */],
    },
    solana: {
      rpc: process.env.SOLANA_RPC_URL,
      listeners: [
        {
          program: 'TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA',
          handler: 'functions/onSolanaTokenTransfer.ts',
        },
      ],
    },
  },
});
```

**Handler function (runs in V8 sandbox):**

```typescript
// functions/onNftTransfer.ts
import { chainEvent } from '@darshjdb/server';

export default chainEvent('Transfer', async (ctx, event) => {
  const { from, to, tokenId } = event.args;

  // Write to triple store -- triggers reactive queries automatically
  await ctx.db.transact([
    ctx.db.tx.nftOwnership[`${event.address}-${tokenId}`].update({
      owner: to,
      previousOwner: from,
      transferredAt: new Date(event.block.timestamp * 1000),
      blockNumber: event.block.number,
      txHash: event.transactionHash,
    }),
  ]);

  // Downstream: any client subscribed to nftOwnership gets a real-time diff
});
```

#### Indexer Integration

For historical data and complex queries across multiple contracts, DarshJDB integrates with The Graph as a data source rather than replacing it.

```typescript
// ddb.config.ts
export default defineConfig({
  indexers: {
    uniswapV3: {
      type: 'subgraph',
      endpoint: 'https://api.thegraph.com/subgraphs/name/uniswap/uniswap-v3',
      syncInterval: '30s',
      entities: ['pools', 'swaps', 'positions'],
      handler: 'functions/syncUniswap.ts',
    },
    custom: {
      type: 'ponder', // or 'envio', 'goldsky'
      endpoint: 'http://localhost:42069',
      syncInterval: '5s',
    },
  },
});
```

The key insight: The Graph provides the data, DarshJDB provides the real-time reactivity, permissions, and offline sync. They complement each other.

#### Chain-State Caching

Every RPC call costs money (Alchemy, Infura) and adds latency. DarshJDB caches on-chain state in PostgreSQL and serves it from there.

```typescript
// Server function: cached chain read
import { chainRead } from '@darshjdb/server';

export const getTokenBalance = chainRead({
  chain: 'ethereum',
  cache: {
    ttl: '12s',         // One Ethereum block
    key: (args) => `${args.token}-${args.wallet}`,
  },
  async handler(ctx, { token, wallet }) {
    // This call hits RPC only if cache is stale
    const balance = await ctx.chain.ethereum.readContract({
      address: token,
      abi: erc20Abi,
      functionName: 'balanceOf',
      args: [wallet],
    });
    return { balance: balance.toString() };
  },
});
```

Cache invalidation strategy:
- **Time-based**: TTL aligned to block time (12s Ethereum, 2s Polygon, 400ms Solana)
- **Event-driven**: Transfer event for a token invalidates balance cache for `from` and `to`
- **Manual**: `ctx.cache.invalidate('token-balance', key)` in server functions

#### Multi-Chain Support

| Chain | Transport | Block Time | Event Model |
|-------|-----------|------------|-------------|
| Ethereum | WebSocket / HTTP JSON-RPC | ~12s | EVM event logs |
| Polygon | WebSocket / HTTP JSON-RPC | ~2s | EVM event logs |
| Arbitrum | WebSocket / HTTP JSON-RPC | ~250ms | EVM event logs |
| Base | WebSocket / HTTP JSON-RPC | ~2s | EVM event logs |
| Solana | WebSocket (accountSubscribe) | ~400ms | Program logs + account changes |

All EVM chains share the same listener code. Solana requires a separate adapter because its programming model (programs + accounts) differs from EVM (contracts + events).

---

## 3. Decentralized Storage Integration

### Problem

DarshJDB's Storage Engine currently supports S3-compatible backends (local FS, S3, R2, MinIO). Web3 applications need content-addressable storage that is censorship-resistant and permanent. IPFS, Arweave, and Filecoin serve different needs -- DarshJDB should support all three as first-class storage backends.

### Design

The Storage Engine abstracts backends behind a `StorageProvider` trait. Adding decentralized backends means implementing this trait for each protocol.

#### Architecture

```
StorageEngine
  |
  |-- S3Provider (existing)
  |-- LocalFSProvider (existing)
  |-- IpfsProvider (new)
  |-- ArweaveProvider (new)
  |-- FilecoinProvider (new)
```

#### IPFS Backend

IPFS is ideal for content-addressable, deduplicated storage. When a file is uploaded, its CID (Content Identifier) becomes the reference.

```typescript
// ddb.config.ts
export default defineConfig({
  storage: {
    default: 's3',
    providers: {
      s3: { /* existing S3 config */ },
      ipfs: {
        type: 'ipfs',
        gateway: 'https://w3s.link',     // Read gateway
        api: 'http://localhost:5001',      // IPFS daemon API
        // Or use web3.storage / Pinata
        pinning: {
          service: 'web3.storage',
          token: process.env.WEB3_STORAGE_TOKEN,
        },
      },
    },
  },
});
```

```typescript
// Client SDK: upload to IPFS
const { cid, url } = await db.storage.upload(file, {
  provider: 'ipfs', // override default
});
// cid = 'bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi'
// url = 'https://w3s.link/ipfs/bafybei...'

// Store CID reference in triple store
await db.transact(
  db.tx.nfts[tokenId].update({
    metadataCid: cid,
    imageUrl: url,
  })
);
```

#### Arweave for Permanent Storage

Arweave guarantees permanent storage with a single upfront payment. Ideal for NFT metadata, legal documents, and audit trails.

```typescript
// Upload to Arweave (permanent)
const { txId, url } = await db.storage.upload(metadata, {
  provider: 'arweave',
  tags: [
    { name: 'Content-Type', value: 'application/json' },
    { name: 'App-Name', value: 'MyDApp' },
  ],
});
// txId = Arweave transaction ID
// url = 'https://arweave.net/{txId}'
```

#### Filecoin for Large Datasets

Filecoin is cost-effective for large datasets (datasets, model weights, archives). DarshJDB integrates via Lighthouse or web3.storage.

```typescript
const { dealId, cid } = await db.storage.upload(largeDataset, {
  provider: 'filecoin',
  replicationFactor: 3,
});
```

#### Content-Addressable References in Triple Store

The triple store (EAV over Postgres) gets a new value type: `cid`. This enables queries that resolve decentralized storage references.

```typescript
// Query NFTs and auto-resolve their IPFS metadata
const { data } = db.useQuery({
  nfts: {
    $where: { collection: '0x1234...abcd' },
    $resolve: ['metadataCid'], // auto-fetch from IPFS and inline
  }
});
// data.nfts[0].metadataCid resolves to the actual JSON content
```

Storage resolution happens server-side with caching. The client never needs to know about IPFS gateways.

---

## 4. Token-Gated Permissions

### Problem

Web3 access control is fundamentally different from Web2. Instead of database-stored roles, permissions derive from on-chain state: Do you hold this NFT? Do you have 100+ governance tokens? Are you a member of this DAO?

### Design

DarshJDB's Permission Engine already evaluates rules at query time via SQL WHERE injection. Token-gated permissions extend the Role Resolution step (step 2 in the pipeline) to query on-chain state.

**Permission evaluation pipeline (extended):**

```
1. Authenticate (JWT -- unchanged)
2. Resolve Roles (database roles + ON-CHAIN ROLES)  <-- new
3. Table Permission
4. Row-Level Security
5. Field Filtering
6. Query Complexity
7. Execute
8. Sanitize Response
```

#### Permission Rules Based on NFT Ownership

```typescript
// permissions.ts
import { definePermissions } from '@darshjdb/server';

export default definePermissions({
  // Only holders of Bored Ape #1234 can access VIP content
  vipContent: {
    read: {
      tokenGate: {
        chain: 'ethereum',
        contract: BAYC_ADDRESS,
        standard: 'ERC-721',
        // Any token in the collection
        condition: 'ownsAny',
      },
    },
  },

  // Specific token ID required
  adminPanel: {
    read: {
      tokenGate: {
        chain: 'ethereum',
        contract: ADMIN_NFT_ADDRESS,
        standard: 'ERC-721',
        condition: { tokenId: 1 },
      },
    },
  },

  // Multiple collections (OR logic)
  premiumFeatures: {
    read: {
      tokenGate: {
        any: [
          { chain: 'ethereum', contract: BAYC_ADDRESS, standard: 'ERC-721' },
          { chain: 'ethereum', contract: MAYC_ADDRESS, standard: 'ERC-721' },
          { chain: 'polygon', contract: LENS_PROFILES, standard: 'ERC-721' },
        ],
      },
    },
  },
});
```

#### ERC-20 Balance-Based Access Control

```typescript
export default definePermissions({
  governanceProposals: {
    // Need 100+ governance tokens to create proposals
    create: {
      tokenGate: {
        chain: 'ethereum',
        contract: GOV_TOKEN_ADDRESS,
        standard: 'ERC-20',
        condition: { minBalance: parseUnits('100', 18) },
      },
    },
    // Need 1+ token to vote (read proposals)
    read: {
      tokenGate: {
        chain: 'ethereum',
        contract: GOV_TOKEN_ADDRESS,
        standard: 'ERC-20',
        condition: { minBalance: parseUnits('1', 18) },
      },
    },
  },
});
```

#### DAO Membership Verification

```typescript
export default definePermissions({
  daoInternal: {
    read: {
      tokenGate: {
        type: 'dao',
        // Snapshot space
        space: 'mydao.eth',
        condition: 'isMember',
      },
    },
    write: {
      tokenGate: {
        type: 'dao',
        space: 'mydao.eth',
        // On-chain governor contract
        governor: GOVERNOR_ADDRESS,
        condition: { role: 'proposer' },
      },
    },
  },
});
```

#### On-Chain Role Resolution

The Role Resolution step becomes async when token gates are involved. To avoid latency on every query, DarshJDB caches on-chain roles with smart invalidation.

```rust
// Rust-side role resolution (pseudocode)
pub async fn resolve_roles(user: &User, ctx: &RequestContext) -> Vec<Role> {
    let mut roles = Vec::new();

    // 1. Database roles (existing, fast)
    roles.extend(db_roles(user).await);

    // 2. On-chain roles (cached, invalidated by chain events)
    if let Some(wallet) = &user.wallet_address {
        let chain_roles = chain_role_cache
            .get_or_fetch(wallet, || async {
                let mut cr = Vec::new();

                // Check NFT holdings
                for gate in &ctx.permission_config.token_gates {
                    if check_token_gate(wallet, gate).await? {
                        cr.push(gate.grants_role.clone());
                    }
                }

                Ok(cr)
            })
            .await?;

        roles.extend(chain_roles);
    }

    roles
}
```

**Cache invalidation:**
- Transfer events for watched contracts invalidate roles for `from` and `to` addresses
- TTL fallback: 60 seconds for ERC-20 balances, 5 minutes for NFT ownership
- Manual invalidation via `db.auth.refreshRoles()` from client

**Full token-gated query example:**

```typescript
// Client side -- no special handling needed
// Permissions are enforced server-side transparently
function DaoForum() {
  const { data, error } = db.useQuery({
    daoInternal: {
      $order: { createdAt: 'desc' },
      $limit: 50,
      author: {}, // join author
    }
  });

  if (error?.code === 'FORBIDDEN') {
    return <MintMembershipNFT />;
  }

  return <ForumPosts posts={data?.daoInternal} />;
}
```

The client code is identical to any other DarshJDB query. The Permission Engine handles token verification transparently. If the user's wallet does not hold the required token, they get a 403 -- same as any other permission denial.

---

## 5. Verifiable Data

### Problem

Web3's trust model is "don't trust, verify." When DarshJDB serves query results, a dApp should be able to prove that the data is authentic and untampered -- without trusting the DarshJDB server.

### Design

#### Merkle Proof Generation for Query Results

DarshJDB's triple store writes are ACID transactions against PostgreSQL. Each transaction produces a deterministic state root using a Merkle tree over the affected entities.

```
                    State Root
                   /          \
            Hash(A..M)      Hash(N..Z)
           /        \       /        \
      Hash(A..F) Hash(G..M) ...     ...
       /    \
   Hash(A) Hash(B)  ...
```

**Query response with proof:**

```typescript
// Client requests verifiable query
const { data, proof } = await db.query({
  nfts: {
    $where: { tokenId: 42 },
  }
}, { withProof: true });

// proof = {
//   root: '0xabc...', // state root
//   leaves: [...],     // entity hashes
//   siblings: [...],   // Merkle siblings
//   blockAnchored: {   // on-chain anchor (if configured)
//     chain: 'ethereum',
//     txHash: '0xdef...',
//     blockNumber: 19234567,
//   }
// }

// Verify client-side
const valid = db.verifyProof(data, proof);
```

#### Data Attestations (EAS Integration)

The Ethereum Attestation Service (EAS) provides a standard for on-chain attestations. DarshJDB can automatically create attestations for critical data.

```typescript
// Server function: attest data on-chain
import { action } from '@darshjdb/server';
import { EAS } from '@ethereum-attestation-service/eas-sdk';

export const attestRecord = action(async (ctx, { entityId, schema }) => {
  const entity = await ctx.db.query({ [schema]: { $where: { id: entityId } } });

  const attestation = await eas.attest({
    schema: DDB_SCHEMA_UID,
    data: {
      recipient: entity.owner,
      data: encodeDarshanAttestation(entity),
      revocable: true,
    },
  });

  // Store attestation reference back in triple store
  await ctx.db.transact(
    ctx.db.tx[schema][entityId].update({
      attestationUid: attestation.uid,
      attestedAt: new Date(),
    })
  );

  return attestation;
});
```

#### Audit Trail On-Chain Anchoring

DarshJDB already logs every mutation with actor, timestamp, and diff (Layer 9: Audit Logging). On-chain anchoring extends this by periodically posting a hash of the audit log to a smart contract.

```
Every N minutes (configurable):
  1. Compute hash of all audit entries since last anchor
  2. Post hash to AuditAnchor smart contract
  3. Store anchor txHash in audit_anchors table
  4. Any audit entry can now be verified against its anchor
```

This provides tamper-evidence without putting raw data on-chain. If someone modifies the database, the audit hashes will not match the on-chain anchors.

#### Zero-Knowledge Proof Verification for Private Queries

For sensitive data, DarshJDB can verify ZK proofs that attest to data properties without revealing the data itself.

```typescript
// Server function: verify a ZK proof about user's age
export const verifyAgeProof = action(async (ctx, { proof, publicSignals }) => {
  // User proves they are over 18 without revealing their age
  const valid = await snarkjs.groth16.verify(
    verificationKey,
    publicSignals,
    proof
  );

  if (valid) {
    // Grant access without storing the actual age
    await ctx.db.transact(
      ctx.db.tx.users[ctx.auth.userId].update({
        ageVerified: true,
        ageProofHash: hashProof(proof),
      })
    );
  }

  return { verified: valid };
});
```

Use cases:
- Prove wallet balance exceeds threshold without revealing exact balance
- Prove DAO membership without revealing which DAO
- Prove credential ownership (KYC) without exposing personal data
- Private voting: prove vote eligibility without linking vote to identity

---

## 6. dApp Backend Pattern

### The Firebase-for-Web3 Architecture

DarshJDB replaces Firebase/Supabase for dApps by providing a unified off-chain layer that stays synchronized with on-chain state.

```
                       +-------------------+
                       |   Smart Contracts  |
                       |   (Source of Truth) |
                       +--------+----------+
                                |
                         Events | State Reads
                                |
                +---------------v--------------+
                |         DarshJDB            |
                |  +-----------------------+   |
                |  | Chain Listener Service |   |  <-- Watches contracts
                |  +-----------+-----------+   |
                |              |               |
                |  +-----------v-----------+   |
                |  |     Triple Store      |   |  <-- Stores off-chain data
                |  |  (EAV over Postgres)  |   |
                |  +-----------+-----------+   |
                |              |               |
                |  +-----------v-----------+   |
                |  |     Sync Engine       |   |  <-- Real-time push
                |  +-----------+-----------+   |
                |              |               |
                |  +-----------v-----------+   |
                |  |  Permission Engine    |   |  <-- Token-gated access
                |  +-----------------------+   |
                +---------------+--------------+
                                |
                     WebSocket  |  MsgPack
                                |
                +---------------v--------------+
                |        dApp Frontend         |
                |  @darshjdb/react + wagmi      |
                +------------------------------+
```

### Off-Chain Data with On-Chain Verification

Most dApp data does not belong on-chain. User profiles, messages, preferences, metadata, analytics -- all of this is off-chain data that references on-chain state. DarshJDB is the off-chain layer.

**Pattern: hybrid data model**

```typescript
// On-chain: NFT ownership (contract state)
// Off-chain: NFT metadata, comments, likes, user profiles

function NftDetailPage({ tokenId }: { tokenId: number }) {
  // DarshJDB query -- pulls from triple store
  // NFT ownership was synced by ChainListener
  const { data } = db.useQuery({
    nfts: {
      $where: { tokenId },
      owner: {},      // user profile (off-chain)
      comments: {     // comments (off-chain)
        $order: { createdAt: 'desc' },
        author: {},
      },
      bids: {         // bid history (synced from chain events)
        $order: { amount: 'desc' },
        bidder: {},
      },
    }
  });

  // This is a LIVE query.
  // When someone comments, or a new bid event is emitted on-chain,
  // this component re-renders automatically.

  return <NftDetail nft={data?.nfts[0]} />;
}
```

### Real-Time Updates from Chain Events

This is where DarshJDB's reactive query engine becomes a superpower for dApps. Chain events write to the triple store, which triggers the Sync Engine, which pushes diffs to subscribed clients.

**End-to-end flow:**

```
1. User places bid on NFT via smart contract
2. Transaction confirmed on Ethereum (~12s)
3. DarshJDB ChainListener receives BidPlaced event
4. Handler writes bid to triple store
5. Sync Engine detects change, matches against subscribed queries
6. All clients viewing that NFT receive a real-time diff
7. UI updates -- no polling, no manual refresh
```

**Latency:**

| Step | Time |
|------|------|
| Transaction confirmation | ~12s (Ethereum), ~2s (Polygon/Base) |
| Event detection + handler | ~100ms |
| Triple store write + sync | ~1ms |
| WebSocket push to client | ~1ms |
| **Total after confirmation** | **~102ms** |

Compare this to the current dApp experience: poll an API every 5 seconds, hope the indexer has caught up, manually refetch. DarshJDB makes chain events feel as instant as database writes.

### Why This Beats the Alternatives

| Capability | Firebase + Moralis | Supabase + Alchemy | DarshJDB |
|-----------|:-----------------:|:------------------:|:---------:|
| Self-hosted | No | Partial | Yes |
| Wallet auth (native) | Plugin | Plugin | Built-in |
| Real-time from chain events | Manual wiring | Manual wiring | Automatic |
| Token-gated permissions | Custom code | Custom code | Declarative rules |
| Offline-first | Limited | No | Yes |
| Single binary | N/A | No (10+ services) | Yes |
| Open source | No | Partial | MIT |

---

## 7. Implementation Roadmap

### Phase 1: Wallet Auth + Token Gates (Weeks 1-6)

**Priority: HIGH -- This unblocks all other Web3 features.**

| Week | Deliverable | Effort |
|------|-------------|--------|
| 1-2 | SIWE (EIP-4361) authentication provider | 2 engineers |
| 2-3 | Solana wallet auth (Ed25519 verification) | 1 engineer |
| 3-4 | Multi-chain identity linking | 1 engineer |
| 4-5 | Token-gated permission rules (NFT + ERC-20) | 2 engineers |
| 5-6 | Client SDK updates (`db.auth.wallet.*`) | 1 engineer |
| 6 | Documentation + example dApp | 1 engineer |

**Crate dependencies (Rust):**
- `siwe` -- SIWE message parsing and verification
- `alloy` -- Ethereum primitives, ABI encoding, contract calls
- `solana-sdk` -- Ed25519 signature verification
- `ethers-core` (or `alloy-primitives`) -- address types, checksums

**Success criteria:**
- `ddb dev` starts with wallet auth enabled
- A React dApp can sign in with MetaMask and query token-gated data
- Permission rules can reference on-chain NFT/token ownership
- All existing auth (email, OAuth, MFA) continues to work unchanged

### Phase 2: On-Chain Sync + Decentralized Storage (Weeks 7-14)

**Priority: MEDIUM -- Makes DarshJDB a complete dApp backend.**

| Week | Deliverable | Effort |
|------|-------------|--------|
| 7-8 | ChainListener service (EVM event subscription) | 2 engineers |
| 9-10 | Chain event handlers in V8 function runtime | 1 engineer |
| 10-11 | Solana program log listener | 1 engineer |
| 11-12 | Chain-state caching layer | 1 engineer |
| 12-13 | IPFS StorageProvider | 1 engineer |
| 13-14 | Arweave + Filecoin providers | 1 engineer |
| 14 | Integration tests + multi-chain example | 1 engineer |

**External dependencies:**
- RPC provider access (Alchemy, Infura, or self-hosted nodes)
- IPFS daemon or web3.storage account
- Arweave wallet with AR tokens for permanent storage

**Success criteria:**
- Chain events on Ethereum/Polygon/Base trigger real-time updates in DarshJDB
- Files uploaded to IPFS/Arweave via `db.storage.upload()` with correct CID references
- Chain state cached in Postgres with smart invalidation

### Phase 3: Verifiable Data + Advanced Features (Weeks 15-22)

**Priority: LOWER -- Differentiator for trust-minimized applications.**

| Week | Deliverable | Effort |
|------|-------------|--------|
| 15-16 | Merkle proof generation for query results | 2 engineers |
| 17-18 | Audit trail on-chain anchoring (smart contract + service) | 2 engineers |
| 19-20 | EAS attestation integration | 1 engineer |
| 20-21 | ZK proof verification in server functions | 1 engineer |
| 21-22 | The Graph subgraph sync adapter | 1 engineer |
| 22 | Security audit of all Web3 features | External firm |

**Success criteria:**
- Query results include optional Merkle proofs
- Audit log hashes are periodically anchored on-chain
- ZK proofs can be verified in server functions
- Full security audit passes with no critical findings

### Dependency Graph

```
Phase 1: Wallet Auth
    |
    +-- Token-Gated Permissions (needs wallet identity)
    |
    v
Phase 2: On-Chain Sync
    |
    +-- Chain Listener (needs RPC connections)
    +-- Decentralized Storage (independent, can parallelize)
    |
    v
Phase 3: Verifiable Data
    |
    +-- Merkle Proofs (needs triple store integration)
    +-- On-Chain Anchoring (needs Chain Listener)
    +-- ZK Verification (needs V8 runtime extensions)
```

### Non-Goals (Explicitly Out of Scope)

- **DarshJDB is not a blockchain.** It does not run consensus, mint tokens, or replace smart contracts.
- **DarshJDB is not an indexer.** It integrates with indexers (The Graph, Ponder) but does not replace them for full-chain indexing.
- **DarshJDB does not store private keys.** Wallet signatures happen client-side. The server only verifies.
- **DarshJDB does not submit transactions.** Server functions can prepare transactions but signing happens in the user's wallet.

---

## Summary

DarshJDB's existing architecture -- reactive queries, triple store, permission engine, V8 sandboxed functions, offline-first sync -- maps almost perfectly onto what dApp developers need. The gap is wallet auth, chain event ingestion, decentralized storage, and on-chain verification. Filling that gap with three focused phases turns DarshJDB into the definitive self-hosted backend for Web3.

The developer in Ahmedabad building a DeFi dashboard, the student in Lagos shipping an NFT marketplace, the freelancer in Sao Paulo launching a DAO tool -- they all need the same thing: a backend that speaks both Web2 and Web3 natively. DarshJDB makes that real.
