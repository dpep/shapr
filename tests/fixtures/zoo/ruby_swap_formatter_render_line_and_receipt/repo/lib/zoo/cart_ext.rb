# frozen_string_literal: true

module Zoo
  class Cart
    def remove(name)
      @items.reject! do |i| i.name == name end
    end
  end
end
