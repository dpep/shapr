# frozen_string_literal: true

module Zoo
  # A shopping cart.
  class Cart
    attr_reader :items

    def initialize(pricing, discount)
      @items = []
      @pricing = pricing
      @discount = discount
    end

    def add(item)
      @items << item
      self
    end

    def remove(name)
      @items.reject! { |i| i.name == name }
    end

    def subtotal
      sum = 0
      @items.each do |item|
        sum += item.subtotal
      end
      sum
    end

    def total(weight)
      discounted = @discount.apply(subtotal)
      @pricing.total(discounted, weight)
    end

    def empty?
      @items.empty?
    end

    def count
      @items.size
    end

    private

    def validate(item)
      raise ArgumentError, 'bad item' if !item.is_a?(Item)
    end
  end
end
