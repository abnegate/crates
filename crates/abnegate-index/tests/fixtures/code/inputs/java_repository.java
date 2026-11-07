package com.acme.widgets.repository;

import java.util.ArrayList;
import java.util.List;
import java.util.Optional;
import org.junit.jupiter.api.Test;

/**
 * Stores customer records in memory.
 */
public class CustomerRepository implements Repository<Customer> {
    private final List<Customer> customers = new ArrayList<>();
    private static final int MAXIMUM_CUSTOMERS = 1000;

    public Optional<Customer> findByEmail(String email) {
        for (Customer customer : customers) {
            if (customer.getEmail().equalsIgnoreCase(email)) {
                return Optional.of(customer);
            }
        }
        return Optional.empty();
    }

    public boolean add(Customer customer) {
        if (customers.size() >= MAXIMUM_CUSTOMERS) {
            return false;
        }
        return customers.add(customer);
    }

    private int count() {
        return customers.size();
    }
}

interface Repository<T> {
    boolean add(T item);
}

enum CustomerTier {
    BRONZE,
    SILVER,
    GOLD
}

class CustomerRepositoryTest {
    @Test
    void addsCustomer() {
        CustomerRepository repository = new CustomerRepository();
        repository.add(new Customer("ada@example.com"));
    }
}
