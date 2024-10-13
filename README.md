# Sourced Rust

Sourced Rust is a Rust implementation of event sourcing patterns, providing a foundation for building event-driven applications.

## Features

- Entity: Represents domain objects with associated events and commands.
- Event Emitter: Manages event publishing and subscription.
- Repository: Handles entity persistence and event storage.

## Project Structure

```
sourced_rust/
├── src/
│   ├── lib.rs
│   ├── entity.rs
│   ├── event_emitter.rs
│   └── repository.rs
├── examples/
│   └── todos/
│       ├── main.rs
│       ├── todo_repository.rs
│       └── todo.rs
├── Cargo.toml
└── README.md
```

## Usage

Add this to your `Cargo.toml`:

```toml
[dependencies]
sourced_rust = { path = "path/to/sourced_rust" }
```

Then, in your Rust code:

```rust
use sourced_rust::{Entity, Event, CommandRecord, EventEmitter, Repository};

// Implement your domain-specific entities, events, and repositories
```

## Example

Check out the `examples/todos` directory for a sample implementation of a todo list application using Sourced Rust.

To run the example:

```
cargo run --example todos
```

## License

This project is open-source and available under the [MIT License](LICENSE).

## Contributing

Contributions are welcome! Please feel free to submit a Pull Request.
g