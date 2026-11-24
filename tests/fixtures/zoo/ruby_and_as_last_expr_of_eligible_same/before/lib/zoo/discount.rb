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

    def member_ok(item, member)
      ok = item.qty > 2 and member
      ok
    end

    def label
      "discount"
    end
  end
end
