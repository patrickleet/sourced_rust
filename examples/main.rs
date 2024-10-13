mod todos {
    pub mod todo;
    pub mod todo_repository;
}

use todos::todo::Todo;
use todos::todo_repository::TodoRepository;

fn main() -> Result<(), String> {
    // Create a new TodoRepository
    let repo = TodoRepository::new();

    // Create a new Todo
    let mut todo = Todo::new();
    todo.initialize("1".to_string(), "user1".to_string(), "Buy groceries".to_string());

    // Add event listeners
    todo.on("ToDoInitialized", |data| {
        match Todo::deserialize(&data) {
            Ok(deserialized_todo) => {
                println!("Todo Initialized: {:?}", deserialized_todo.snapshot());
            },
            Err(e) => {
                println!("Error deserializing Todo: {}", e);
            }
        }
    });
    
    todo.on("ToDoCompleted", |data| {
        match Todo::deserialize(&data) {
            Ok(deserialized_todo) => {
                println!("Todo Completed: {:?}", deserialized_todo.snapshot());
            },
            Err(e) => {
                println!("Error deserializing Todo: {}", e);
            }
        }
    });

    // Commit the Todo to the repository
    repo.commit(&mut todo)?;

    // Retrieve the Todo from the repository
    match repo.find_by_id("1") {
        Some(retrieved_todo) => {
            println!("Retrieved Todo: {:?}", retrieved_todo);

            // retrieved_todo.complete();
        }
        None => {
            println!("Todo not found");
        }
    }

    Ok(())
}
