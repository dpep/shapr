# frozen_string_literal: true

module Zoo
  # One purchasable line in a cart, with quantity.
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
