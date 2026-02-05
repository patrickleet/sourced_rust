use super::Entity;

/// Trait for types that can be committed to a repository.
pub trait Committable {
    /// Returns mutable references to all entities to be committed.
    fn entities_mut(&mut self) -> Vec<&mut Entity>;
}

// Single Entity
impl Committable for Entity {
    fn entities_mut(&mut self) -> Vec<&mut Entity> {
        vec![self]
    }
}

// Slice of mutable Entity references
impl<'a> Committable for [&'a mut Entity] {
    fn entities_mut(&mut self) -> Vec<&mut Entity> {
        self.iter_mut().map(|e| &mut **e).collect()
    }
}

// Fixed-size arrays for common cases
impl<'a, const N: usize> Committable for [&'a mut Entity; N] {
    fn entities_mut(&mut self) -> Vec<&mut Entity> {
        self.iter_mut().map(|e| &mut **e).collect()
    }
}

// Vec of mutable Entity references
impl<'a> Committable for Vec<&'a mut Entity> {
    fn entities_mut(&mut self) -> Vec<&mut Entity> {
        self.iter_mut().map(|e| &mut **e).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_entity() {
        let mut entity = Entity::with_id("test");
        let entities = entity.entities_mut();
        assert_eq!(entities.len(), 1);
        assert_eq!(entities[0].id(), "test");
    }

    #[test]
    fn array_of_entities() {
        let mut a = Entity::with_id("a");
        let mut b = Entity::with_id("b");
        let mut arr = [&mut a, &mut b];
        let entities = arr.entities_mut();
        assert_eq!(entities.len(), 2);
        assert_eq!(entities[0].id(), "a");
        assert_eq!(entities[1].id(), "b");
    }

    #[test]
    fn vec_of_entities() {
        let mut a = Entity::with_id("a");
        let mut b = Entity::with_id("b");
        let mut c = Entity::with_id("c");
        let mut vec = vec![&mut a, &mut b, &mut c];
        let entities = vec.entities_mut();
        assert_eq!(entities.len(), 3);
    }
}
