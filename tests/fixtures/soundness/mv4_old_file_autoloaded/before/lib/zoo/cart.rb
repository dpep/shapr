# frozen_string_literal: true

module Zoo
  class Cart < Base
    def add(item)
      @items << item
      self
    end

    def remove(name)
      @items.reject! { |i| i.name == name }
    end

    def size
      @items.size
    end
  end
end
