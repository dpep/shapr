# frozen_string_literal: true

module Zoo
  # Edge cases.
  class Traps
    attr_reader :log

    def initialize(cfg = nil)
      @log = []
      @cfg = cfg
    end

    def paren_assign(a, b)
      x = a and b
      x
    end

    def paren_call(a)
      puts a + 1 * 2
    end

    def paren_not(a, b)
      !a == b
    end

    def block_bind(items)
      puts items.map do |i| i.to_s end
    end

    def and_call(x, y)
      puts x && y
    end

    def esc_string
      'a\nb'
    end

    def heredoc
      <<~TXT
        # not a comment
        hello
      TXT
    end

    def words
      %w[a #b c]
    end

    def multiline_str
      "line one
      line two"
    end

    def regex
      /a # keep
        b/x
    end

    def endless_a(x)
      x
    end

    def shadow
      x = 1
      [2].each { |y| x += y }
      x
    end
  end
end
