#include <cstddef>
#include <stdexcept>
#include <vector>

namespace widgets {

template <typename T>
class Matrix {
public:
    Matrix(std::size_t rows, std::size_t columns)
        : rows_(rows), columns_(columns), values_(rows * columns) {}

    T& at(std::size_t row, std::size_t column) {
        if (row >= rows_ || column >= columns_) {
            throw std::out_of_range("matrix index");
        }
        return values_[row * columns_ + column];
    }

    Matrix multiply(const Matrix& other) const {
        Matrix result(rows_, other.columns_);
        for (std::size_t i = 0; i < rows_; ++i) {
            for (std::size_t j = 0; j < other.columns_; ++j) {
                T sum{};
                for (std::size_t k = 0; k < columns_; ++k) {
                    sum += values_[i * columns_ + k] * other.values_[k * other.columns_ + j];
                }
                result.values_[i * other.columns_ + j] = sum;
            }
        }
        return result;
    }

private:
    std::size_t rows_;
    std::size_t columns_;
    std::vector<T> values_;
};

struct Shape {
    std::size_t rows;
    std::size_t columns;
};

Shape identity_shape(std::size_t size) {
    return Shape{size, size};
}

}  // namespace widgets
