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
    todo.entity.on("ToDoInitialized", |data| {
        match Todo::deserialize(&data) {
            Ok(deserialized_todo) => {
                println!("Todo Initialized: {:?}", deserialized_todo.snapshot());
            },
            Err(e) => {
                println!("Error deserializing Todo: {}", e);
            }
        }
    });
    
    // Commit the Todo to the repository
    repo.commit(&mut todo)?;

    // Retrieve the Todo from the repository
    if let Some(mut retrieved_todo) = repo.get("1") {
        println!("Retrieved Todo: {:?}", retrieved_todo);

        retrieved_todo.entity.on("ToDoCompleted", |data| {
            match Todo::deserialize(&data) {
                Ok(deserialized_todo) => {
                    println!("Todo Completed: {:?}", deserialized_todo.snapshot());
                },
                Err(e) => {
                    println!("Error deserializing Todo: {}", e);
                }
            }
        });

        // Complete the Todo
        retrieved_todo.complete();

        // Commit the changes
        repo.commit(&mut retrieved_todo)?;

        // Retrieve the Todo again to demonstrate that events are fired on retrieval
        if let Some(updated_todo) = repo.get("1") {
            println!("Updated Todo: {:?}", updated_todo.snapshot());
        } else {
            println!("Updated Todo not found");
        }
    } else {
        println!("Todo not found");
    }

    let mut todo2 = Todo::new();
    todo2.initialize("2".to_string(), "user1".to_string(), "Buy Sauna".to_string());

    let mut todo3 = Todo::new();
    todo3.initialize("3".to_string(), "user2".to_string(), "Chew bubblegum".to_string()); 


    // Commit multiple Todos to the repository
    repo.commit_all(&mut [&mut todo2, &mut todo3])?;

    // get all the todos from the repository
    let all_todos = repo.get_all(&["1", "2", "3"]);
    if all_todos.len() > 0 {
        println!("All Todos: {:?}", all_todos);
    } else {
        println!("No Todos found");
    }

    Ok(())
}
