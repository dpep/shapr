# frozen_string_literal: true

module Zoo
  class Collide
    def sum_with(a)
      [2, 3].map { |b| a + b }
    end

    def hits
      @hits ||= 0
      @hits += 1
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
