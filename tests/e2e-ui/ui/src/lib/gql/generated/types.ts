export type Maybe<T> = T | null;
export type InputMaybe<T> = Maybe<T>;
/** All built-in and custom scalars, mapped to their actual values */
export type Scalars = {
  ID: { input: string; output: string; }
  String: { input: string; output: string; }
  Boolean: { input: boolean; output: boolean; }
  Int: { input: number; output: number; }
  Float: { input: number; output: number; }
  BigInt: { input: unknown; output: unknown; }
  Bytea: { input: unknown; output: unknown; }
  JSON: { input: unknown; output: unknown; }
  Timestamptz: { input: unknown; output: unknown; }
};

export type BigInt_Comparison_Exp = {
  _eq?: InputMaybe<Scalars['BigInt']['input']>;
  _gt?: InputMaybe<Scalars['BigInt']['input']>;
  _gte?: InputMaybe<Scalars['BigInt']['input']>;
  _in?: InputMaybe<Array<Scalars['BigInt']['input']>>;
  _is_null?: InputMaybe<Scalars['Boolean']['input']>;
  _lt?: InputMaybe<Scalars['BigInt']['input']>;
  _lte?: InputMaybe<Scalars['BigInt']['input']>;
  _neq?: InputMaybe<Scalars['BigInt']['input']>;
  _nin?: InputMaybe<Array<Scalars['BigInt']['input']>>;
};

export type Bytea_Comparison_Exp = {
  _eq?: InputMaybe<Scalars['Bytea']['input']>;
  _gt?: InputMaybe<Scalars['Bytea']['input']>;
  _gte?: InputMaybe<Scalars['Bytea']['input']>;
  _in?: InputMaybe<Array<Scalars['Bytea']['input']>>;
  _is_null?: InputMaybe<Scalars['Boolean']['input']>;
  _lt?: InputMaybe<Scalars['Bytea']['input']>;
  _lte?: InputMaybe<Scalars['Bytea']['input']>;
  _neq?: InputMaybe<Scalars['Bytea']['input']>;
  _nin?: InputMaybe<Array<Scalars['Bytea']['input']>>;
};

export type ChatMessageView = {
  __typename?: 'ChatMessageView';
  author_id: Scalars['String']['output'];
  body: Scalars['String']['output'];
  created_at: Scalars['String']['output'];
  message_id: Scalars['String']['output'];
  room_id: Scalars['String']['output'];
};

export type ChatPostInput = {
  body: Scalars['String']['input'];
  message_id: Scalars['String']['input'];
  room_id: Scalars['String']['input'];
};

export type ChatPostPayload = {
  __typename?: 'ChatPostPayload';
  author_id: Scalars['String']['output'];
  body: Scalars['String']['output'];
  created_at: Scalars['String']['output'];
  message_id: Scalars['String']['output'];
  room_id: Scalars['String']['output'];
};

export type Json_Comparison_Exp = {
  _eq?: InputMaybe<Scalars['JSON']['input']>;
  _gt?: InputMaybe<Scalars['JSON']['input']>;
  _gte?: InputMaybe<Scalars['JSON']['input']>;
  _in?: InputMaybe<Array<Scalars['JSON']['input']>>;
  _is_null?: InputMaybe<Scalars['Boolean']['input']>;
  _lt?: InputMaybe<Scalars['JSON']['input']>;
  _lte?: InputMaybe<Scalars['JSON']['input']>;
  _neq?: InputMaybe<Scalars['JSON']['input']>;
  _nin?: InputMaybe<Array<Scalars['JSON']['input']>>;
};

export type Mutation = {
  __typename?: 'Mutation';
  chat_messages_post: ChatPostPayload;
  todos_archive: TodoArchivePayload;
  todos_complete: TodoStatusPayload;
  todos_create: TodoCreatePayload;
  todos_force_archive: TodoForceArchivePayload;
  todos_rename: TodoRenamePayload;
  todos_reopen: TodoReopenPayload;
};


export type MutationChat_Messages_PostArgs = {
  input: ChatPostInput;
};


export type MutationTodos_ArchiveArgs = {
  input: TodoArchiveInput;
};


export type MutationTodos_CompleteArgs = {
  input: TodoCompleteInput;
};


export type MutationTodos_CreateArgs = {
  input: TodoCreateInput;
};


export type MutationTodos_Force_ArchiveArgs = {
  input: TodoForceArchiveInput;
};


export type MutationTodos_RenameArgs = {
  input: TodoRenameInput;
};


export type MutationTodos_ReopenArgs = {
  input: TodoReopenInput;
};

export type Query = {
  __typename?: 'Query';
  chat_messages: Array<ChatMessageView>;
  chat_messages_by_pk?: Maybe<ChatMessageView>;
  todos: Array<TodoView>;
  todos_by_pk?: Maybe<TodoView>;
};


export type QueryChat_MessagesArgs = {
  limit?: InputMaybe<Scalars['Int']['input']>;
  offset?: InputMaybe<Scalars['Int']['input']>;
  order_by?: InputMaybe<Array<Chat_Messages_Order_By>>;
  where?: InputMaybe<Chat_Messages_Bool_Exp>;
};


export type QueryChat_Messages_By_PkArgs = {
  message_id: Scalars['String']['input'];
};


export type QueryTodosArgs = {
  limit?: InputMaybe<Scalars['Int']['input']>;
  offset?: InputMaybe<Scalars['Int']['input']>;
  order_by?: InputMaybe<Array<Todos_Order_By>>;
  where?: InputMaybe<Todos_Bool_Exp>;
};


export type QueryTodos_By_PkArgs = {
  todo_id: Scalars['String']['input'];
};

export type String_Comparison_Exp = {
  _eq?: InputMaybe<Scalars['String']['input']>;
  _gt?: InputMaybe<Scalars['String']['input']>;
  _gte?: InputMaybe<Scalars['String']['input']>;
  _ilike?: InputMaybe<Scalars['String']['input']>;
  _in?: InputMaybe<Array<Scalars['String']['input']>>;
  _is_null?: InputMaybe<Scalars['Boolean']['input']>;
  _like?: InputMaybe<Scalars['String']['input']>;
  _lt?: InputMaybe<Scalars['String']['input']>;
  _lte?: InputMaybe<Scalars['String']['input']>;
  _neq?: InputMaybe<Scalars['String']['input']>;
  _nin?: InputMaybe<Array<Scalars['String']['input']>>;
};

export type Subscription = {
  __typename?: 'Subscription';
  chat_messages: Array<ChatMessageView>;
  todos: Array<TodoView>;
};


export type SubscriptionChat_MessagesArgs = {
  limit?: InputMaybe<Scalars['Int']['input']>;
  offset?: InputMaybe<Scalars['Int']['input']>;
  order_by?: InputMaybe<Array<Chat_Messages_Order_By>>;
  where?: InputMaybe<Chat_Messages_Bool_Exp>;
};


export type SubscriptionTodosArgs = {
  limit?: InputMaybe<Scalars['Int']['input']>;
  offset?: InputMaybe<Scalars['Int']['input']>;
  order_by?: InputMaybe<Array<Todos_Order_By>>;
  where?: InputMaybe<Todos_Bool_Exp>;
};

export type Timestamptz_Comparison_Exp = {
  _eq?: InputMaybe<Scalars['Timestamptz']['input']>;
  _gt?: InputMaybe<Scalars['Timestamptz']['input']>;
  _gte?: InputMaybe<Scalars['Timestamptz']['input']>;
  _in?: InputMaybe<Array<Scalars['Timestamptz']['input']>>;
  _is_null?: InputMaybe<Scalars['Boolean']['input']>;
  _lt?: InputMaybe<Scalars['Timestamptz']['input']>;
  _lte?: InputMaybe<Scalars['Timestamptz']['input']>;
  _neq?: InputMaybe<Scalars['Timestamptz']['input']>;
  _nin?: InputMaybe<Array<Scalars['Timestamptz']['input']>>;
};

export type TodoArchiveInput = {
  todo_id: Scalars['String']['input'];
};

export type TodoArchivePayload = {
  __typename?: 'TodoArchivePayload';
  status: Scalars['String']['output'];
  todo_id: Scalars['String']['output'];
};

export type TodoCompleteInput = {
  todo_id: Scalars['String']['input'];
};

export type TodoCreateInput = {
  title: Scalars['String']['input'];
  todo_id: Scalars['String']['input'];
};

export type TodoCreatePayload = {
  __typename?: 'TodoCreatePayload';
  owner_id: Scalars['String']['output'];
  status: Scalars['String']['output'];
  title: Scalars['String']['output'];
  todo_id: Scalars['String']['output'];
};

export type TodoForceArchiveInput = {
  todo_id: Scalars['String']['input'];
};

export type TodoForceArchivePayload = {
  __typename?: 'TodoForceArchivePayload';
  archived_by: Scalars['String']['output'];
  owner_id: Scalars['String']['output'];
  status: Scalars['String']['output'];
  todo_id: Scalars['String']['output'];
};

export type TodoRenameInput = {
  title: Scalars['String']['input'];
  todo_id: Scalars['String']['input'];
};

export type TodoRenamePayload = {
  __typename?: 'TodoRenamePayload';
  status: Scalars['String']['output'];
  title: Scalars['String']['output'];
  todo_id: Scalars['String']['output'];
};

export type TodoReopenInput = {
  todo_id: Scalars['String']['input'];
};

export type TodoReopenPayload = {
  __typename?: 'TodoReopenPayload';
  status: Scalars['String']['output'];
  todo_id: Scalars['String']['output'];
};

export type TodoStatusPayload = {
  __typename?: 'TodoStatusPayload';
  status: Scalars['String']['output'];
  todo_id: Scalars['String']['output'];
};

export type TodoView = {
  __typename?: 'TodoView';
  owner_id: Scalars['String']['output'];
  status: Scalars['String']['output'];
  title: Scalars['String']['output'];
  todo_id: Scalars['String']['output'];
};

export type Chat_Messages_Bool_Exp = {
  _and?: InputMaybe<Array<Chat_Messages_Bool_Exp>>;
  _not?: InputMaybe<Chat_Messages_Bool_Exp>;
  _or?: InputMaybe<Array<Chat_Messages_Bool_Exp>>;
  author_id?: InputMaybe<String_Comparison_Exp>;
  body?: InputMaybe<String_Comparison_Exp>;
  created_at?: InputMaybe<String_Comparison_Exp>;
  message_id?: InputMaybe<String_Comparison_Exp>;
  room_id?: InputMaybe<String_Comparison_Exp>;
};

export type Chat_Messages_Order_By = {
  author_id?: InputMaybe<Order_By>;
  body?: InputMaybe<Order_By>;
  created_at?: InputMaybe<Order_By>;
  message_id?: InputMaybe<Order_By>;
  room_id?: InputMaybe<Order_By>;
};

export enum Order_By {
  Asc = 'asc',
  AscNullsFirst = 'asc_nulls_first',
  AscNullsLast = 'asc_nulls_last',
  Desc = 'desc',
  DescNullsFirst = 'desc_nulls_first',
  DescNullsLast = 'desc_nulls_last'
}

export type Todos_Bool_Exp = {
  _and?: InputMaybe<Array<Todos_Bool_Exp>>;
  _not?: InputMaybe<Todos_Bool_Exp>;
  _or?: InputMaybe<Array<Todos_Bool_Exp>>;
  owner_id?: InputMaybe<String_Comparison_Exp>;
  status?: InputMaybe<String_Comparison_Exp>;
  title?: InputMaybe<String_Comparison_Exp>;
  todo_id?: InputMaybe<String_Comparison_Exp>;
};

export type Todos_Order_By = {
  owner_id?: InputMaybe<Order_By>;
  status?: InputMaybe<Order_By>;
  title?: InputMaybe<Order_By>;
  todo_id?: InputMaybe<Order_By>;
};
