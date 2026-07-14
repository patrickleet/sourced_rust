export type Maybe<T> = T | null;
export type InputMaybe<T> = Maybe<T>;
/** All built-in and custom scalars, mapped to their actual values */
export type Scalars = {
  ID: { input: string; output: string; }
  String: { input: string; output: string; }
  Boolean: { input: boolean; output: boolean; }
  Int: { input: number; output: number; }
  Float: { input: number; output: number; }
  JSON: { input: unknown; output: unknown; }
};

export type ChatMessage = {
  __typename?: 'ChatMessage';
  author_id: Scalars['String']['output'];
  body: Scalars['String']['output'];
  created_at: Scalars['String']['output'];
  message_id: Scalars['String']['output'];
  room_id: Scalars['String']['output'];
};

export type ChatMessagesPostInput = {
  body: Scalars['String']['input'];
  message_id: Scalars['String']['input'];
  room_id: Scalars['String']['input'];
};

export type ChatMessagesWhere = {
  room_id?: InputMaybe<StringEq>;
};

export type Mutation = {
  __typename?: 'Mutation';
  chat_messages_post?: Maybe<ChatMessage>;
  todos_archive?: Maybe<TodoStatusPayload>;
  todos_complete?: Maybe<TodoStatusPayload>;
  todos_create?: Maybe<TodoCreatePayload>;
  todos_rename?: Maybe<TodoCreatePayload>;
  todos_reopen?: Maybe<TodoStatusPayload>;
};


export type MutationChat_Messages_PostArgs = {
  input: ChatMessagesPostInput;
};


export type MutationTodos_ArchiveArgs = {
  input: TodosArchiveInput;
};


export type MutationTodos_CompleteArgs = {
  input: TodosCompleteInput;
};


export type MutationTodos_CreateArgs = {
  input: TodosCreateInput;
};


export type MutationTodos_RenameArgs = {
  input: TodosRenameInput;
};


export type MutationTodos_ReopenArgs = {
  input: TodosReopenInput;
};

export type Query = {
  __typename?: 'Query';
  chat_messages: Array<ChatMessage>;
  todos: Array<Todo>;
};


export type QueryChat_MessagesArgs = {
  where?: InputMaybe<ChatMessagesWhere>;
};

export type StringEq = {
  _eq?: InputMaybe<Scalars['String']['input']>;
};

export type Subscription = {
  __typename?: 'Subscription';
  chat_messages: Array<ChatMessage>;
  todos: Array<Todo>;
};


export type SubscriptionChat_MessagesArgs = {
  where?: InputMaybe<ChatMessagesWhere>;
};

export type Todo = {
  __typename?: 'Todo';
  owner_id: Scalars['String']['output'];
  status: Scalars['String']['output'];
  title: Scalars['String']['output'];
  todo_id: Scalars['String']['output'];
};

export type TodoCreatePayload = {
  __typename?: 'TodoCreatePayload';
  owner_id: Scalars['String']['output'];
  status: Scalars['String']['output'];
  title: Scalars['String']['output'];
  todo_id: Scalars['String']['output'];
};

export type TodoStatusPayload = {
  __typename?: 'TodoStatusPayload';
  status: Scalars['String']['output'];
  todo_id: Scalars['String']['output'];
};

export type TodosArchiveInput = {
  todo_id: Scalars['String']['input'];
};

export type TodosCompleteInput = {
  todo_id: Scalars['String']['input'];
};

export type TodosCreateInput = {
  title: Scalars['String']['input'];
  todo_id: Scalars['String']['input'];
};

export type TodosRenameInput = {
  title: Scalars['String']['input'];
  todo_id: Scalars['String']['input'];
};

export type TodosReopenInput = {
  todo_id: Scalars['String']['input'];
};
