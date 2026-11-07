// Package catalog serves the product catalog over HTTP.
package catalog

import (
	"encoding/json"
	"net/http"
	"sync"
	"testing"
)

// Product is one catalog entry.
type Product struct {
	ID    string  `json:"id"`
	Name  string  `json:"name"`
	Price float64 `json:"price"`
}

// Store keeps products in memory.
type Store struct {
	mu       sync.RWMutex
	products map[string]Product
}

// Lister lists products.
type Lister interface {
	List() []Product
}

// NewStore creates an empty store.
func NewStore() *Store {
	return &Store{products: make(map[string]Product)}
}

// Put inserts or replaces a product.
func (s *Store) Put(product Product) {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.products[product.ID] = product
}

// List returns every product.
func (s *Store) List() []Product {
	s.mu.RLock()
	defer s.mu.RUnlock()
	out := make([]Product, 0, len(s.products))
	for _, product := range s.products {
		out = append(out, product)
	}
	return out
}

func handleList(lister Lister) http.HandlerFunc {
	return func(writer http.ResponseWriter, request *http.Request) {
		writer.Header().Set("Content-Type", "application/json")
		_ = json.NewEncoder(writer).Encode(lister.List())
	}
}

func TestStorePutThenList(t *testing.T) {
	store := NewStore()
	store.Put(Product{ID: "a", Name: "Anchor", Price: 3.5})
	if len(store.List()) != 1 {
		t.Fatal("expected one product")
	}
}
