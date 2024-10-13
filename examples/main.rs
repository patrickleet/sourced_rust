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
    todo.on("ToDoInitialized".to_string(), |data| {
        if let Some(todo_instance) = data.downcast_ref::<Todo>() {
            println!("Todo Initialized: {:?}", todo_instance.snapshot());
        } else {
            println!("Error casting event data");
        }
    });

    todo.on("ToDoCompleted".to_string(), |data| {
        println!("Todo completed: {:?}", data);
    });

    // Commit the Todo to the repository
    repo.commit(&mut todo)?;

    // Complete the Todo
    todo.complete();

    // Commit the changes
    repo.commit(&mut todo)?;

    // Retrieve the Todo from the repository
    match repo.find_by_id("1") {
        Some(retrieved_todo) => {
            println!("Retrieved Todo: {:?}", retrieved_todo.snapshot());
        }
        None => {
            println!("Todo not found");
        }
    }

    Ok(())
}
