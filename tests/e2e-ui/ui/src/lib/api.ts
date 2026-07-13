import { identityHeaders, readSession, type Session } from './session';

function base(): string {
  if (typeof window !== 'undefined') {
    const w = window as unknown as { E2E_BASE_URL?: string };
    if (w.E2E_BASE_URL) return w.E2E_BASE_URL;
  }
  return '';
}

export type Todo = {
  todo_id: string;
  owner_id: string;
  title: string;
  status: string;
};

export async function listTodos(session?: Session): Promise<Todo[]> {
  const s = session ?? readSession();
  const res = await fetch(`${base()}/graphql`, {
    method: 'POST',
    headers: identityHeaders(s),
    body: JSON.stringify({
      query: `{ todos { todo_id owner_id title status } }`,
    }),
  });
  if (!res.ok) throw new Error(`GraphQL HTTP ${res.status}`);
  const body = await res.json();
  if (body.errors?.length) throw new Error(body.errors[0].message);
  return body.data?.todos ?? [];
}

export async function createTodo(title: string, todoId: string, session?: Session) {
  const s = session ?? readSession();
  const res = await fetch(`${base()}/todo.create`, {
    method: 'POST',
    headers: identityHeaders(s),
    body: JSON.stringify({ todo_id: todoId, title }),
  });
  const body = await res.json().catch(() => ({}));
  if (!res.ok) throw new Error(body.error ?? `HTTP ${res.status}`);
  return body;
}

export async function completeTodo(todoId: string, session?: Session) {
  const s = session ?? readSession();
  const res = await fetch(`${base()}/todo.complete`, {
    method: 'POST',
    headers: identityHeaders(s),
    body: JSON.stringify({ todo_id: todoId }),
  });
  const body = await res.json().catch(() => ({}));
  if (!res.ok) throw new Error(body.error ?? `HTTP ${res.status}`);
  return body;
}

export async function archiveTodo(todoId: string, session?: Session) {
  const s = session ?? readSession();
  const res = await fetch(`${base()}/todo.archive`, {
    method: 'POST',
    headers: identityHeaders(s),
    body: JSON.stringify({ todo_id: todoId }),
  });
  const body = await res.json().catch(() => ({}));
  if (!res.ok) throw new Error(body.error ?? `HTTP ${res.status}`);
  return body;
}
