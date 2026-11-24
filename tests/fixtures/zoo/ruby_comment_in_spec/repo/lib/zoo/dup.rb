# frozen_string_literal: true

module Zoo
  class Dup
    def x
      2
    end

    def x
      1
    end
  end
end
