# frozen_string_literal: true

module Zoo
  # Discount rules.
  class Discount
    def initialize(percent)
      @percent = percent
    end

    def apply( amount )
      return amount if @percent.zero?


      amount - ( amount * @percent / 100.0 )
    end

    def eligible?(item, member)
      item.qty > 2 && member
    end

    def label
      "discount"
    end
  end
end
