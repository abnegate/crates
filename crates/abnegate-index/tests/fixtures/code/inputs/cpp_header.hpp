#pragma once

#include <string>
#include <memory>

namespace widgets {

class Renderer {
public:
    virtual ~Renderer() = default;
    virtual void draw(const std::string& label) = 0;
    virtual std::size_t frames() const = 0;
};

class ConsoleRenderer : public Renderer {
public:
    void draw(const std::string& label) override;
    std::size_t frames() const override { return frames_; }

private:
    std::size_t frames_ = 0;
};

std::unique_ptr<Renderer> make_renderer(bool console);

}  // namespace widgets
