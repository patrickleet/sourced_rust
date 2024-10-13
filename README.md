# Sourced Rust

Sourced Rust is a robust Rust implementation of event sourcing patterns, providing a solid foundation for building event-driven applications. This library empowers developers to create scalable, maintainable, and auditable systems by leveraging the power of event sourcing.

## Project Inspiration

Sourced Rust is inspired by the original [sourced](https://github.com/mateodelnorte/sourced) project by Matt Walters. Patrick Lee Scott, a contributor and maintainer of the original JavaScript/TypeScript version, has brought these concepts to Rust, extending and refactoring it for the Rust ecosystem.

## What is Event Sourcing?

Event sourcing is an architectural pattern where the state of your application is determined by a sequence of events. Instead of storing just the current state, event sourcing systems store all changes to the application state as a sequence of events. This approach offers several benefits:

- Complete Audit Trail: Every change is recorded and can be audited.
- Temporal Query: You can determine the state of the application at any point in time.
- Event Replay: You can replay events to recreate the state or to test new business logic against historical data.

## Features

Sourced Rust provides several key components to implement event sourcing in your Rust applications:

- **Entity**: Represents domain objects with associated events and commands. Entities are the core of your domain model and encapsulate business logic.
- **Event Emitter**: Manages event publishing and subscription. This allows for loose coupling between components and enables reactive programming patterns. From event-emitter-rs.
- **Repository**: Handles entity persistence and event storage. It provides an abstraction layer over the storage mechanism, making it easy to switch between different storage solutions.
- **Event**: Represents something that has happened in the domain. Events are immutable and represent facts.

## Installation

Add this to your `Cargo.toml`:

```toml
[dependencies]
sourced_rust = { git = "https://github.com/patrickleet/sourced_rust.git" }
```

## Usage

Here's a basic example of how to use Sourced Rust in your project:

```rust
use sourced_rust::{Entity, Event, EventRecord, EventEmitter, Repository};

// Define your domain-specific entity
struct Todo {
    id: String,
    title: String,
    completed: bool,
}

// Implement the Entity trait for your struct
impl Entity for Todo {
    // Implement required methods
}

// Define your events
enum TodoEvent {
    Created { title: String },
    Completed,
}

// Implement event handling logic
impl Todo {
    fn apply(&mut self, event: TodoEvent) {
        match event {
            TodoEvent::Created { title } => {
                self.title = title;
                self.completed = false;
            },
            TodoEvent::Completed => {
                self.completed = true;
            },
        }
    }
}

// Use the Repository to store and retrieve entities
let mut repo = TodoRepository::new();
let todo = Todo::new("1", "Buy milk");
repo.save(&todo);

// Emit events
let emitter = EventEmitter::new();
emitter.emit(TodoEvent::Created { title: "Buy milk".to_string() });
```

## Project Structure

```
sourced_rust/
├── src/
│   ├── lib.rs         # Library root, exports public API
│   ├── entity.rs      # Entity trait definition
│   └── repository.rs  # Repository trait and implementation
├── examples/
│   ├── main.rs        # Example usage
│   └── todos/         # Todo list example
│       ├── todo_repository.rs
│       └── todo.rs
├── Cargo.toml         # Project manifest
└── README.md          # This file
```

## Running Tests

To run the test suite, use the following command:

```
cargo test
```

## Examples

Check out the `examples/todos` directory for a sample implementation of a todo list application using Sourced Rust. This example demonstrates how to create entities, define events, and use the repository pattern with event sourcing.

To run the example:

```
cargo run --example todos
```

## Project Status

Sourced Rust is currently in active development. We are working on expanding the feature set and improving performance. Contributions and feedback are welcome!

## Roadmap

- Implement snapshotting for faster entity rebuilding
- Add support for event versioning and upcasting
- Integrate with popular databases for event storage
- Develop more comprehensive examples and documentation

## Reporting Issues

If you encounter any bugs or have feature requests, please file an issue on the [GitHub issue tracker](https://github.com/patrickleet/sourced_rust/issues).

## License

This project is open-source and available under the [MIT License](LICENSE).

## Contributing

Contributions are welcome! Please feel free to submit a Pull Request. For major changes, please open an issue first to discuss what you would like to change.

1. Fork the Project
2. Create your Feature Branch (`git checkout -b feature/AmazingFeature`)
3. Commit your Changes (`git commit -m 'Add some AmazingFeature'`)
4. Push to the Branch (`git push origin feature/AmazingFeature`)
5. Open a Pull Request

Thank you to all the contributors who have helped to make Sourced Rust better!
