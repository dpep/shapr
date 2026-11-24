# frozen_string_literal: true

module Zoo
  class Traps2
    def first_of(list)
      a = list
      a
    end

    def joined(list)
      puts list.map do |x| x.to_s end.join(",")
    end

    def rescued(list)
      v = (Integer(list) rescue 0)
      v
    end

    def cond(c)
      x = c ? 1 : 2
      x
    end

    def loopy(n)
      n -= 1 while n > 0
      n
    end

    def assign_if(c)
      y = 5 if c
      y
    end

    def chain(list)
      list.each { |i| puts i }.size
    end

    def neg(x)
      -x.abs
    end

    def pow
      -2 ** 2
    end

    def nilsafe(o)
      o&.name
    end

    def lam
      ->(x) { x + 1 }
    end

    def str_cat
      'a' 'b'
    end

    def sym_hash(k)
      { 'k': k }
    end
  end
end
