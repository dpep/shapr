# frozen_string_literal: true

module Zoo
  class Collide
    def sum_with(b)
      [2, 3].map { |b| b + b }
    end

    def hits
      @tally ||= 0
      @tally += 1
    end

    def bump
      1
    end
  end

  class SubCollide < Collide
    def tally
      @tally = 100
      hits + @tally
    end

    def peek
      42
    end
  end
end
