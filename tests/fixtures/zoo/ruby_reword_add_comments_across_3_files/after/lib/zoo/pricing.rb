# frozen_string_literal: true

module Zoo
  # Computes taxes and shipping.
  class Pricing
    TAX_RATE = 0.08

    def initialize(region)
      @region = region
      @calls = 0
    end

    # Tax is flat-rate per region.
    def tax(amount)
      @calls += 1
      amount * TAX_RATE
    end

    def shipping(weight, express = false)
      base = weight * 2
      if express
        base * 3
      else
        base
      end
    end

    def total(amount, weight)
      t = tax(amount)
      s = shipping(weight)
      amount + t + s
    end

    def calls
      @calls
    end

    private # helpers

    def log(msg)
      @log ||= []
      @log << msg
    end

    def region_code
      @region.to_s.upcase
    end
  end
end
