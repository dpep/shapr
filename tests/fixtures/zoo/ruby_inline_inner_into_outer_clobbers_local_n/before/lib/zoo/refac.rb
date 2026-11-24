# frozen_string_literal: true

module Zoo
  class Refac
    def run(obj)
      obj.instance_eval do
        setup
      end
    end

    def setup
      @a = 1
      @b = 2
      @c = @a + @b
    end

    def stamp_it
      ts = ts
      "at #{ts}"
    end

    def ts
      Time.now.to_s.strip.squeeze.downcase.reverse
    end

    def outer
      n = 10
      q = 3
      m = inner(q)
      n + m
    end

    def inner(k)
      n = k * 2
      n + 1
    end
  end
end
