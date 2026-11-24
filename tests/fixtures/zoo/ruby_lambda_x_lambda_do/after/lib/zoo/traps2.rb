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
      while n > 0 do n -= 1 end
      n
    end

    def assign_if(c)
      if c then y = 5 end
      y
    end

    def chain(list)
      list.each do |i| puts i end.size
    end

    def neg(x)
      -x.abs
    end

    def pow
      (-2) ** 2
    end

    def nilsafe(o)
      o.name
    end

    def lam
      lambda { |x| x + 1 }
    end

    def str_cat
      'a' 'b'
    end

    def sym_hash(k)
      { 'k': k }
    end
  end
end
