# frozen_string_literal: true

module Zoo
  # A line item.
  class Item
    attr_reader :name, :price, :qty

    def initialize(name, price, qty = 1)
      @name = name
      @price = price
      @qty = qty
    end

    def subtotal
      price * qty
    end

    def to_h
      { :name => name, :price => price, :qty => qty }
    end
  end
end
