# frozen_string_literal: true

module Zoo
  class Dup
    def x
      1
    end

    def x
      2
    end
  end
end
