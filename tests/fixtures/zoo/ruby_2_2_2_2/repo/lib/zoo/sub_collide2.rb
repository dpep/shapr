# frozen_string_literal: true

module Zoo
  class SubCollide2 < Pricing
    def pong
      42
    end

    def total_extra
      @extra = 5
      @extra + 1
    end
  end
end
