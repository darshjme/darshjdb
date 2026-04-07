# Sharing & Collaboration

DarshJDB provides three collaboration mechanisms: **share links** for anonymous or semi-authenticated access via URL tokens, **collaborators** for named-user access with role-based permissions, and **workspaces** for grouping resources under a shared permission boundary.

## Share Links

Share links generate short, URL-friendly tokens (8 characters, base62-encoded) that grant scoped access to a specific resource.

### Create a Share Link

```bash
curl -X POST http://localhost:3000/api/shares \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d '{
    "resource_type": "table",
    "resource_id": "550e8400-e29b-41d4-a716-446655440000",
    "permission": "read_only",
    "password": "optional-secret",
    "expires_at": "2026-05-01T00:00:00Z",
    "max_uses": 100
  }'
```

Response:

```json
{
  "id": "share-uuid",
  "token": "aB3xK9mQ",
  "resource_type": "table",
  "resource_id": "550e8400-...",
  "permission": "read_only",
  "expires_at": "2026-05-01T00:00:00Z",
  "max_uses": 100,
  "use_count": 0,
  "created_at": "2026-04-07T10:00:00Z"
}
```

Share the URL: `https://your-instance.com/s/aB3xK9mQ`

### Permission Levels

| Permission | Capabilities |
|------------|-------------|
| `read_only` | View the resource |
| `comment` | View and add comments |
| `edit` | View, comment, and modify data |

Permissions are ordered: `read_only < comment < edit`.

### Resource Types

| Type | Description |
|------|-------------|
| `table` | An entire entity type / collection |
| `view` | A specific view (filtered/sorted subset) |
| `record` | A single record |

### Password Protection

When a password is set during creation, the password is stored as a hash (BCrypt or Argon2). Clients accessing the share link must supply the password to gain access. Without the correct password, the link resolves to `None`.

### Expiring Shares

Set `expires_at` to an ISO-8601 timestamp. After expiry, the share link returns null on resolution. Omit or set to `null` for a link that never expires.

### Usage Caps

Set `max_uses` to limit how many times the link can be accessed. Once `use_count` reaches `max_uses`, the link becomes inactive. Each successful resolution increments the counter atomically.

### Resolve a Share Token

```bash
curl http://localhost:3000/api/shares/resolve/{token}
```

Returns the share configuration if the token is valid, not expired, not revoked, and has not exceeded its usage cap. Returns 404 otherwise.

### Revoke a Share Link

```bash
curl -X POST http://localhost:3000/api/shares/{share_id}/revoke \
  -H "Authorization: Bearer <token>"
```

Revoked links can never be resolved again.

### List Share Links

```bash
curl http://localhost:3000/api/shares \
  -H "Authorization: Bearer <token>"
```

Returns all non-revoked share links created by the authenticated user.

## Collaborator Invites and Roles

The collaborator system manages named-user access to specific resources with a strict role hierarchy.

### Role Hierarchy

```
Owner > Admin > Editor > Commenter > Viewer
```

| Role | Can View | Can Comment | Can Edit | Can Manage Members |
|------|----------|-------------|----------|-------------------|
| Viewer | Yes | No | No | No |
| Commenter | Yes | Yes | No | No |
| Editor | Yes | Yes | Yes | No |
| Admin | Yes | Yes | Yes | Yes (below Admin) |
| Owner | Yes | Yes | Yes | Yes (all) |

Management rules:
- Only `Owner` and `Admin` can invite, remove, or change roles.
- An `Admin` can modify `Editor`, `Commenter`, and `Viewer` roles but cannot modify another `Admin` or the `Owner`.
- Only the `Owner` can modify `Admin` roles.
- You cannot invite someone as `Owner` -- use ownership transfer instead.

### Invite a Collaborator

```bash
curl -X POST http://localhost:3000/api/collaborators \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d '{
    "email": "bob@example.com",
    "resource_type": "table",
    "resource_id": "550e8400-...",
    "role": "editor"
  }'
```

Response:

```json
{
  "id": "collaborator-uuid",
  "user_id": null,
  "email": "bob@example.com",
  "resource_type": "table",
  "resource_id": "550e8400-...",
  "role": "editor",
  "status": "pending",
  "invited_by": "owner-uuid",
  "invited_at": "2026-04-07T10:00:00Z",
  "accepted_at": null
}
```

The invite starts in `pending` status. `user_id` is `null` until the invitee accepts.

### Accept an Invite

```bash
curl -X POST http://localhost:3000/api/collaborators/{collaborator_id}/accept \
  -H "Authorization: Bearer <token>"
```

Binds the collaborator record to the authenticated user's ID and sets status to `accepted`. Only pending invites can be accepted.

### Invite Statuses

| Status | Description |
|--------|-------------|
| `pending` | Invite sent, awaiting acceptance |
| `accepted` | User accepted and has access |
| `declined` | User declined the invite |
| `revoked` | Access was revoked by an admin/owner |

### Update a Collaborator's Role

```bash
curl -X PATCH http://localhost:3000/api/collaborators/{collaborator_id}/role \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d '{ "role": "commenter" }'
```

Cannot promote to `owner`.

### Remove a Collaborator

```bash
curl -X DELETE http://localhost:3000/api/collaborators/{collaborator_id} \
  -H "Authorization: Bearer <token>"
```

Sets the collaborator's status to `revoked`.

### List Collaborators for a Resource

```bash
curl "http://localhost:3000/api/collaborators?resource_type=table&resource_id=550e8400-..." \
  -H "Authorization: Bearer <token>"
```

Returns all active (non-revoked) collaborators.

### Check a User's Role

```bash
curl "http://localhost:3000/api/collaborators/role?user_id=...&resource_type=table&resource_id=..." \
  -H "Authorization: Bearer <token>"
```

Returns the user's role for the specified resource, or 404 if they have no access.

## Workspaces

A workspace is a container that groups tables and views under a shared permission boundary. Resources within a workspace inherit the workspace-level member roles unless overridden at the resource level via the collaborator system.

### Create a Workspace

```bash
curl -X POST http://localhost:3000/api/workspaces \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "Engineering Team",
    "settings": {
      "default_role": "editor",
      "branding": { "color": "#1a1a2e" }
    }
  }'
```

Response:

```json
{
  "id": "workspace-uuid",
  "name": "Engineering Team",
  "slug": "engineering-team",
  "owner_id": "creator-uuid",
  "created_at": "2026-04-07T10:00:00Z",
  "settings": {
    "default_role": "editor",
    "branding": { "color": "#1a1a2e" }
  }
}
```

The slug is auto-generated from the name: lowercased, non-alphanumeric characters replaced with hyphens, consecutive hyphens collapsed. The creating user is automatically added as `Owner`.

### Workspace Roles

Workspace roles mirror the collaborator hierarchy:

```
Owner > Admin > Editor > Commenter > Viewer
```

Same management rules apply: `Owner` and `Admin` can manage members; `Admin` cannot modify other `Admin` or `Owner` roles.

### Update a Workspace

```bash
curl -X PATCH http://localhost:3000/api/workspaces/{workspace_id} \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "Platform Team",
    "settings": { "default_role": "viewer" }
  }'
```

Updating the name automatically regenerates the slug.

### Add a Member

```bash
curl -X POST http://localhost:3000/api/workspaces/{workspace_id}/members \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d '{
    "user_id": "new-member-uuid",
    "role": "editor"
  }'
```

### Update a Member's Role

```bash
curl -X PATCH http://localhost:3000/api/workspaces/{workspace_id}/members/{member_id}/role \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d '{ "role": "admin" }'
```

### Remove a Member

```bash
curl -X DELETE http://localhost:3000/api/workspaces/{workspace_id}/members/{member_id} \
  -H "Authorization: Bearer <token>"
```

Soft-deletes the membership by setting an `active` flag to `false`.

### List Workspace Members

```bash
curl http://localhost:3000/api/workspaces/{workspace_id}/members \
  -H "Authorization: Bearer <token>"
```

Response:

```json
{
  "members": [
    {
      "id": "member-uuid",
      "user_id": "user-uuid",
      "workspace_id": "workspace-uuid",
      "role": "owner",
      "joined_at": "2026-04-07T10:00:00Z"
    }
  ]
}
```

### List User's Workspaces

```bash
curl http://localhost:3000/api/workspaces \
  -H "Authorization: Bearer <token>"
```

Returns all workspaces the authenticated user belongs to.

### Get a Workspace

```bash
curl http://localhost:3000/api/workspaces/{workspace_id} \
  -H "Authorization: Bearer <token>"
```

### Check Member Role

```bash
curl "http://localhost:3000/api/workspaces/{workspace_id}/members/role?user_id=..." \
  -H "Authorization: Bearer <token>"
```

Returns the user's role within the workspace, or 404 if they are not a member.
