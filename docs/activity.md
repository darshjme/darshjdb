# Comments & Activity

DarshJDB provides threaded comments on records, an append-only activity log capturing all mutations, and an in-app notification system for mentions, replies, assignments, and shares.

## Threaded Comments

Comments are attached to records (entities) and support arbitrary nesting via `reply_to` threading. Comment bodies support Markdown.

### Create a Comment

```bash
curl -X POST http://localhost:3000/api/records/{entity_id}/comments \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d '{
    "content": "Looks good, but check the pricing on line 3.",
    "mentions": ["550e8400-e29b-41d4-a716-446655440001"],
    "reply_to": null
  }'
```

Response (201 Created):

```json
{
  "id": "comment-uuid",
  "entity_id": "record-uuid",
  "user_id": "author-uuid",
  "content": "Looks good, but check the pricing on line 3.",
  "mentions": ["550e8400-e29b-41d4-a716-446655440001"],
  "reply_to": null,
  "created_at": "2026-04-07T10:00:00Z",
  "updated_at": "2026-04-07T10:00:00Z",
  "deleted": false
}
```

Validation rules:
- Content must not be empty.
- If `reply_to` is provided, the parent comment must exist, not be deleted, and belong to the same entity.

### Reply to a Comment

```bash
curl -X POST http://localhost:3000/api/records/{entity_id}/comments \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d '{
    "content": "Fixed. See updated pricing.",
    "reply_to": "parent-comment-uuid"
  }'
```

### List Comments (Threaded)

```bash
curl http://localhost:3000/api/records/{entity_id}/comments \
  -H "Authorization: Bearer <token>"
```

Response returns a threaded tree. Top-level comments (no `reply_to`) form roots, with nested `replies` arrays:

```json
[
  {
    "id": "root-comment-uuid",
    "entity_id": "record-uuid",
    "user_id": "user-1",
    "content": "Looks good, but check the pricing.",
    "mentions": [],
    "reply_to": null,
    "created_at": "2026-04-07T10:00:00Z",
    "updated_at": "2026-04-07T10:00:00Z",
    "deleted": false,
    "replies": [
      {
        "id": "reply-uuid",
        "entity_id": "record-uuid",
        "user_id": "user-2",
        "content": "Fixed. See updated pricing.",
        "mentions": [],
        "reply_to": "root-comment-uuid",
        "created_at": "2026-04-07T10:05:00Z",
        "updated_at": "2026-04-07T10:05:00Z",
        "deleted": false,
        "replies": []
      }
    ]
  }
]
```

Comments are ordered by `created_at` ascending within each level. Soft-deleted comments are excluded from listing, and their orphaned replies are also hidden.

### Update a Comment

Only the comment author can edit:

```bash
curl -X PATCH http://localhost:3000/api/comments/{comment_id} \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d '{
    "content": "Updated text here.",
    "mentions": ["new-mention-uuid"]
  }'
```

### Delete a Comment

Soft-delete (only the author can delete):

```bash
curl -X DELETE http://localhost:3000/api/comments/{comment_id} \
  -H "Authorization: Bearer <token>"
```

The comment remains in the database with `deleted: true` for audit purposes.

## Activity Log

Every mutation, comment, share, and link operation is recorded as an `ActivityEntry` in the append-only `activity_log` table. Entries are never modified after insertion.

### Activity Actions

| Action | Description |
|--------|-------------|
| `created` | A new record was created |
| `updated` | An existing record was updated |
| `deleted` | A record was deleted |
| `commented` | A comment was added to a record |
| `shared` | A record was shared with another user |
| `linked_record` | A reference link was established between records |
| `unlinked_record` | A reference link was removed |

### Activity Entry Structure

```json
{
  "id": "activity-uuid",
  "entity_type": "user",
  "entity_id": "record-uuid",
  "action": "updated",
  "user_id": "actor-uuid",
  "timestamp": "2026-04-07T10:00:00Z",
  "changes": [
    {
      "field_name": "name",
      "old_value": "Alice",
      "new_value": "Alice Smith"
    },
    {
      "field_name": "phone",
      "old_value": null,
      "new_value": "+1234567890"
    }
  ],
  "metadata": { "source": "api" }
}
```

Field-level changes are computed automatically by diffing old and new attribute maps. For actions like `commented` and `shared`, the `changes` array is empty and context is in `metadata`.

### Get Activity for a Record

```bash
curl "http://localhost:3000/api/records/{entity_id}/activity?limit=50" \
  -H "Authorization: Bearer <token>"
```

Returns entries in reverse chronological order (newest first).

### Get Activity by User

```bash
curl "http://localhost:3000/api/users/{user_id}/activity?limit=50" \
  -H "Authorization: Bearer <token>"
```

### Get Activity for an Entity Type

```bash
curl "http://localhost:3000/api/tables/{entity_type}/activity?limit=50" \
  -H "Authorization: Bearer <token>"
```

## Notification System

Notifications are generated automatically when users are mentioned, replied to, assigned records, or receive shared resources. Each notification links back to the resource that triggered it.

### Notification Kinds

| Kind | Trigger |
|------|---------|
| `mention` | User was `@mentioned` in a comment |
| `reply` | Someone replied to the user's comment |
| `assignment` | A record was assigned to the user |
| `share` | A record or table was shared with the user |
| `system_alert` | System-level alert (e.g., quota warning, schema migration) |

Self-notifications are suppressed: mentioning yourself, replying to your own comment, or sharing with yourself does not generate a notification.

### Notification Structure

```json
{
  "id": "notification-uuid",
  "user_id": "recipient-uuid",
  "kind": "mention",
  "title": "You were mentioned in a comment",
  "body": "Looks good, but check the pricing on line 3...",
  "resource_type": "comment",
  "resource_id": "comment-uuid",
  "read": false,
  "created_at": "2026-04-07T10:00:00Z"
}
```

### Get Notifications

```bash
# All notifications
curl http://localhost:3000/api/notifications \
  -H "Authorization: Bearer <token>"

# Unread only
curl "http://localhost:3000/api/notifications?unread=true" \
  -H "Authorization: Bearer <token>"
```

Returns up to 100 notifications in reverse chronological order.

### Mark a Notification as Read

```bash
curl -X POST http://localhost:3000/api/notifications/{notification_id}/read \
  -H "Authorization: Bearer <token>"
```

### Mark All as Read

```bash
curl -X POST http://localhost:3000/api/notifications/read-all \
  -H "Authorization: Bearer <token>"
```

Response:

```json
{ "marked_read": 12 }
```

### Unread Count

```bash
curl http://localhost:3000/api/notifications/unread-count \
  -H "Authorization: Bearer <token>"
```

Response:

```json
{ "unread": 5 }
```

This endpoint is designed for badge rendering in client UIs.
