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

export type AuthUserView = {
  __typename?: 'AuthUserView';
  approval_status: Scalars['String']['output'];
  blob_games: Array<BlobGameView>;
  chat_messages: Array<ChatMessageView>;
  display_name: Scalars['String']['output'];
  email: Scalars['String']['output'];
  status: Scalars['String']['output'];
  updated_at: Scalars['String']['output'];
  user_id: Scalars['String']['output'];
  user_kind: Scalars['String']['output'];
};


export type AuthUserViewBlob_GamesArgs = {
  limit?: InputMaybe<Scalars['Int']['input']>;
  offset?: InputMaybe<Scalars['Int']['input']>;
  order_by?: InputMaybe<Array<Blob_Games_Order_By>>;
  where?: InputMaybe<Blob_Games_Bool_Exp>;
};


export type AuthUserViewChat_MessagesArgs = {
  limit?: InputMaybe<Scalars['Int']['input']>;
  offset?: InputMaybe<Scalars['Int']['input']>;
  order_by?: InputMaybe<Array<Chat_Messages_Order_By>>;
  where?: InputMaybe<Chat_Messages_Bool_Exp>;
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

export type BlobGamePayload = {
  __typename?: 'BlobGamePayload';
  current_level: Scalars['BigInt']['output'];
  current_level_completed: Scalars['Boolean']['output'];
  game_id: Scalars['String']['output'];
  map_json: Scalars['String']['output'];
  owner_id: Scalars['String']['output'];
  player_dead: Scalars['Boolean']['output'];
  score: Scalars['BigInt']['output'];
  status: Scalars['String']['output'];
};

export type BlobGameView = {
  __typename?: 'BlobGameView';
  current_level: Scalars['BigInt']['output'];
  current_level_completed: Scalars['Boolean']['output'];
  game_id: Scalars['String']['output'];
  map_json: Scalars['String']['output'];
  owner: AuthUserView;
  owner_id: Scalars['String']['output'];
  player_dead: Scalars['Boolean']['output'];
  score: Scalars['BigInt']['output'];
  status: Scalars['String']['output'];
};

export type BlobMoveInput = {
  direction: Scalars['String']['input'];
  game_id: Scalars['String']['input'];
};

export type BlobStartInput = {
  game_id: Scalars['String']['input'];
};

export type BlobStartLevelInput = {
  game_id: Scalars['String']['input'];
};

export type Boolean_Comparison_Exp = {
  _eq?: InputMaybe<Scalars['Boolean']['input']>;
  _gt?: InputMaybe<Scalars['Boolean']['input']>;
  _gte?: InputMaybe<Scalars['Boolean']['input']>;
  _in?: InputMaybe<Array<Scalars['Boolean']['input']>>;
  _is_null?: InputMaybe<Scalars['Boolean']['input']>;
  _lt?: InputMaybe<Scalars['Boolean']['input']>;
  _lte?: InputMaybe<Scalars['Boolean']['input']>;
  _neq?: InputMaybe<Scalars['Boolean']['input']>;
  _nin?: InputMaybe<Array<Scalars['Boolean']['input']>>;
};

export type ChatMessageView = {
  __typename?: 'ChatMessageView';
  author: AuthUserView;
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

export type Mutation = {
  __typename?: 'Mutation';
  blob_games_move: BlobGamePayload;
  blob_games_start: BlobGamePayload;
  blob_games_start_level: BlobGamePayload;
  chat_messages_post: ChatPostPayload;
  todos_archive: TodoStatusPayload;
  todos_complete: TodoStatusPayload;
  todos_create: TodoCreatePayload;
  todos_force_archive: TodoForceArchivePayload;
  todos_rename: TodoRenamePayload;
  todos_reopen: TodoStatusPayload;
};


export type MutationBlob_Games_MoveArgs = {
  input: BlobMoveInput;
};


export type MutationBlob_Games_StartArgs = {
  input: BlobStartInput;
};


export type MutationBlob_Games_Start_LevelArgs = {
  input: BlobStartLevelInput;
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
  auth_users: Array<AuthUserView>;
  auth_users_by_pk?: Maybe<AuthUserView>;
  blob_games: Array<BlobGameView>;
  blob_games_by_pk?: Maybe<BlobGameView>;
  chat_messages: Array<ChatMessageView>;
  chat_messages_by_pk?: Maybe<ChatMessageView>;
  todos: Array<TodoView>;
  todos_by_pk?: Maybe<TodoView>;
};


export type QueryAuth_UsersArgs = {
  limit?: InputMaybe<Scalars['Int']['input']>;
  offset?: InputMaybe<Scalars['Int']['input']>;
  order_by?: InputMaybe<Array<Auth_Users_Order_By>>;
  where?: InputMaybe<Auth_Users_Bool_Exp>;
};


export type QueryAuth_Users_By_PkArgs = {
  user_id: Scalars['String']['input'];
};


export type QueryBlob_GamesArgs = {
  limit?: InputMaybe<Scalars['Int']['input']>;
  offset?: InputMaybe<Scalars['Int']['input']>;
  order_by?: InputMaybe<Array<Blob_Games_Order_By>>;
  where?: InputMaybe<Blob_Games_Bool_Exp>;
};


export type QueryBlob_Games_By_PkArgs = {
  game_id: Scalars['String']['input'];
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
  auth_users: Array<AuthUserView>;
  blob_games: Array<BlobGameView>;
  chat_messages: Array<ChatMessageView>;
  todos: Array<TodoView>;
};


export type SubscriptionAuth_UsersArgs = {
  limit?: InputMaybe<Scalars['Int']['input']>;
  offset?: InputMaybe<Scalars['Int']['input']>;
  order_by?: InputMaybe<Array<Auth_Users_Order_By>>;
  where?: InputMaybe<Auth_Users_Bool_Exp>;
};


export type SubscriptionBlob_GamesArgs = {
  limit?: InputMaybe<Scalars['Int']['input']>;
  offset?: InputMaybe<Scalars['Int']['input']>;
  order_by?: InputMaybe<Array<Blob_Games_Order_By>>;
  where?: InputMaybe<Blob_Games_Bool_Exp>;
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

export type TodoArchiveInput = {
  todo_id: Scalars['String']['input'];
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

export type Auth_Users_Bool_Exp = {
  _and?: InputMaybe<Array<Auth_Users_Bool_Exp>>;
  _not?: InputMaybe<Auth_Users_Bool_Exp>;
  _or?: InputMaybe<Array<Auth_Users_Bool_Exp>>;
  approval_status?: InputMaybe<String_Comparison_Exp>;
  blob_games?: InputMaybe<Blob_Games_Bool_Exp>;
  chat_messages?: InputMaybe<Chat_Messages_Bool_Exp>;
  display_name?: InputMaybe<String_Comparison_Exp>;
  email?: InputMaybe<String_Comparison_Exp>;
  status?: InputMaybe<String_Comparison_Exp>;
  updated_at?: InputMaybe<String_Comparison_Exp>;
  user_id?: InputMaybe<String_Comparison_Exp>;
  user_kind?: InputMaybe<String_Comparison_Exp>;
};

export type Auth_Users_Order_By = {
  approval_status?: InputMaybe<Order_By>;
  display_name?: InputMaybe<Order_By>;
  email?: InputMaybe<Order_By>;
  status?: InputMaybe<Order_By>;
  updated_at?: InputMaybe<Order_By>;
  user_id?: InputMaybe<Order_By>;
  user_kind?: InputMaybe<Order_By>;
};

export type Blob_Games_Bool_Exp = {
  _and?: InputMaybe<Array<Blob_Games_Bool_Exp>>;
  _not?: InputMaybe<Blob_Games_Bool_Exp>;
  _or?: InputMaybe<Array<Blob_Games_Bool_Exp>>;
  current_level?: InputMaybe<BigInt_Comparison_Exp>;
  current_level_completed?: InputMaybe<Boolean_Comparison_Exp>;
  game_id?: InputMaybe<String_Comparison_Exp>;
  map_json?: InputMaybe<String_Comparison_Exp>;
  owner?: InputMaybe<Auth_Users_Bool_Exp>;
  owner_id?: InputMaybe<String_Comparison_Exp>;
  player_dead?: InputMaybe<Boolean_Comparison_Exp>;
  score?: InputMaybe<BigInt_Comparison_Exp>;
  status?: InputMaybe<String_Comparison_Exp>;
};

export type Blob_Games_Order_By = {
  current_level?: InputMaybe<Order_By>;
  current_level_completed?: InputMaybe<Order_By>;
  game_id?: InputMaybe<Order_By>;
  map_json?: InputMaybe<Order_By>;
  owner_id?: InputMaybe<Order_By>;
  player_dead?: InputMaybe<Order_By>;
  score?: InputMaybe<Order_By>;
  status?: InputMaybe<Order_By>;
};

export type Chat_Messages_Bool_Exp = {
  _and?: InputMaybe<Array<Chat_Messages_Bool_Exp>>;
  _not?: InputMaybe<Chat_Messages_Bool_Exp>;
  _or?: InputMaybe<Array<Chat_Messages_Bool_Exp>>;
  author?: InputMaybe<Auth_Users_Bool_Exp>;
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
