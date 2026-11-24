# frozen_string_literal: true

module Zoo
  class Sub2 < Base2
    def hi
      super + '!'
    end
  end
end
