# frozen_string_literal: true

module Zoo
  class Cart
    def remove(name)
      @items.reject! { |i| i.name == name }
    end
  end
end
