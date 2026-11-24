# frozen_string_literal: true

module Zoo
  # Computes taxes and shipping.
  class Pricing
    SALES_TAX_RATE = 0.08

    def initialize(region)
      @region = region
      @extra = 0
    end

    def shipping(weight,
                 express = true)
      cost = weight * 2
      if express
        cost
      else
        cost * 4
      end
    end

    def tax(amount)
      @extra += 1
      amount * SALES_TAX_RATE
    end

    def total(amount, weight)
      ship = shipping(weight)
      tax_amount = tax(amount)
      amount + tax_amount + ship
    end

    def pong
      region_key
    end

    def calls
      @extra
    end

    private

    def append_log(msg)
      @log ||= []
      @log << msg
    end

    def region_key
      @region.to_s.upcase
    end
  end
end
