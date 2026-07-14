/**
 * Shared GraphQL documents — used by SSR load and the browser.
 * Keep selection sets identical so hydrate / invalidate stay consistent.
 */

export const TODOS_QUERY = `{
  todos {
    todo_id
    owner_id
    title
    status
  }
}`;

export const TODOS_CREATE = `mutation TodosCreate($todo_id: String!, $title: String!) {
  todos_create(input: { todo_id: $todo_id, title: $title }) {
    todo_id
    owner_id
    title
    status
  }
}`;

export const TODOS_COMPLETE = `mutation TodosComplete($todo_id: String!) {
  todos_complete(input: { todo_id: $todo_id }) {
    todo_id
    status
  }
}`;

export const TODOS_ARCHIVE = `mutation TodosArchive($todo_id: String!) {
  todos_archive(input: { todo_id: $todo_id }) {
    todo_id
    status
  }
}`;

export function chatMessagesQuery(room: string): string {
  return `{
  chat_messages(where: { room_id: { _eq: "${room}" } }) {
    message_id
    room_id
    author_id
    body
    created_at
  }
}`;
}

export function chatMessagesSubscription(room: string): string {
  return `subscription {
  chat_messages(where: { room_id: { _eq: "${room}" } }) {
    message_id
    room_id
    author_id
    body
    created_at
  }
}`;
}

export const CHAT_POST = `mutation ChatPost($message_id: String!, $body: String!, $room_id: String!) {
  chat_messages_post(input: {
    message_id: $message_id
    body: $body
    room_id: $room_id
  }) {
    message_id
    room_id
    author_id
    body
    created_at
  }
}`;
