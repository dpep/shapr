# frozen_string_literal: true

module Zoo
  # Rebate rules.
  class Rebate
    def initialize(percent)
      @percent = percent
    end

    def apply( amount )
      return amount if @percent.zero?


      off = amount * @percent / 100.0
      amount - off
    end

    def eligible?(item, member)
      item.qty > 2 and member
    end

    def member_ok(item, member)
      ok = item.qty > 2 and member
      ok
    end
  end
end
