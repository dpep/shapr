# frozen_string_literal: true

module Zoo
  class SubPricing < Pricing
    def peek
      @call_count
    end
  end
end
